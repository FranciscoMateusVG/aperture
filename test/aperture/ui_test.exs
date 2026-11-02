defmodule Aperture.UITest do
  @moduledoc """
  The launcher page's backend: page, snapshot, live updates (SSE), tmux focus,
  operator inbox, and the local-only guard.
  """
  use ExUnit.Case, async: false

  alias Aperture.{AgentFixtures, FakeAgent, Launcher, Tmux}

  defp port, do: Application.fetch_env!(:aperture, :port)

  # one HTTP request; `headers` lets a test pretend to be another site
  defp request(method, path, body \\ nil, headers \\ []) do
    {:ok, conn} = Mint.HTTP.connect(:http, "127.0.0.1", port(), mode: :passive)
    hdrs = [{"content-type", "application/json"} | headers]
    {:ok, conn, ref} = Mint.HTTP.request(conn, method, path, hdrs, body && JSON.encode!(body))
    {status, data} = recv(conn, ref, nil, "")
    Mint.HTTP.close(conn)
    {status, data}
  end

  defp recv(conn, ref, status, acc) do
    {:ok, conn, responses} = Mint.HTTP.recv(conn, 0, 5_000)

    {status, acc, done} =
      Enum.reduce(responses, {status, acc, false}, fn
        {:status, ^ref, s}, {_, a, d} -> {s, a, d}
        {:data, ^ref, d}, {s, a, done} -> {s, a <> d, done}
        {:done, ^ref}, {s, a, _} -> {s, a, true}
        _, x -> x
      end)

    if done, do: {status, acc}, else: recv(conn, ref, status, acc)
  end

  defp state do
    {200, body} = request("GET", "/ui/state")
    JSON.decode!(body)
  end

  defp agent(snap, name), do: Enum.find(snap["agents"], &(&1["name"] == name))

  test "the launcher page (LiveView) is served at /, with its assets" do
    {200, html} = request("GET", "/")
    assert html =~ ~s[<title data-default="Aperture">]
    assert html =~ "data-phx-main"

    for asset <- ~w(/assets/app.js /assets/app.css /assets/vendor/phoenix.min.js
                    /assets/vendor/phoenix_live_view.min.js),
        do: assert({200, _} = request("GET", asset))

    assert {403, _} = request("GET", "/", nil, [{"origin", "https://evil.example"}])
  end

  test "the installable app's files: manifest, service worker, offline page, icons" do
    {200, manifest} = request("GET", "/manifest.webmanifest")

    assert %{"name" => "Aperture", "start_url" => "/", "display" => "standalone"} =
             JSON.decode!(manifest)

    {200, sw} = request("GET", "/sw.js")
    assert sw =~ "/offline.html"
    {200, offline} = request("GET", "/offline.html")
    assert offline =~ "Aperture está desligado"

    for icon <- JSON.decode!(manifest)["icons"],
        do: assert({200, _} = request("GET", icon["src"]))

    # only those files: nothing else under priv/static, no traversal
    assert {404, _} = request("GET", "/app.js")

    for path <- ["/icons/../../mix.exs", "/icons/%2e%2e/%2e%2e/mix.exs"] do
      assert {status, _} = request("GET", path)
      assert status in [400, 404]
    end
  end

  test "the snapshot lists each agent with harness, model and live state" do
    name =
      AgentFixtures.define(AgentFixtures.unique("u"), %{
        "harness" => "codex",
        "model" => "gpt-6-astra"
      })

    assert %{"state" => "offline", "harness" => "codex", "model" => "gpt-6-astra", "unread" => 0} =
             agent(state(), name)

    _ = FakeAgent.start(name)
    assert %{"state" => "idle"} = agent(state(), name)
    assert state()["harnesses"] == ["claude", "codex"]
    assert %{"claude" => ["opus" | _], "codex" => ["gpt-6-astra" | _]} = state()["models"]
  end

  test "Stop and Start take an agent offline and bring it back" do
    name = AgentFixtures.define(AgentFixtures.unique("t"))
    session = Application.fetch_env!(:aperture, :tmux_session)

    assert {202, _} = request("POST", "/agents/#{name}/start")

    FakeAgent.wait_until(
      fn -> agent(state(), name)["state"] != "offline" end,
      "never online",
      8_000
    )

    assert {202, _} = request("POST", "/agents/#{name}/stop")

    FakeAgent.wait_until(
      fn -> agent(state(), name)["state"] == "offline" end,
      "still online",
      8_000
    )

    refute name in Tmux.windows(session)

    assert {202, _} = request("POST", "/agents/#{name}/start")

    FakeAgent.wait_until(
      fn -> agent(state(), name)["state"] != "offline" end,
      "never back",
      8_000
    )

    assert {404, _} = request("POST", "/agents/nobody/stop")
  end

  test "messages to the operator land in the inbox with a badge, until marked read" do
    name = AgentFixtures.define(AgentFixtures.unique("b"))
    fa = FakeAgent.start(name)

    FakeAgent.send_message(fa, "operator", "need your decision on the deploy")
    FakeAgent.wait_until(fn -> agent(state(), name)["unread"] == 1 end, "no badge")

    snap = state()

    assert [%{"id" => id, "body" => "need your decision on the deploy"}] =
             Enum.filter(snap["inbox"], &(&1["from"] == name))

    assert {200, _} = request("POST", "/operator/ack", %{id: id})
    assert agent(state(), name)["unread"] == 0
    refute Enum.any?(state()["inbox"], &(&1["id"] == id))
  end

  test "the operator can message an agent from the page" do
    name = AgentFixtures.define(AgentFixtures.unique("o"))
    fa = FakeAgent.start(name)

    assert {200, _} =
             request("POST", "/messages", %{from: "operator", to: name, body: "hi from Francisco"})

    assert FakeAgent.wait_for(fa, 1) == ["hi from Francisco"]
  end

  test "live updates: the event stream sends a fresh snapshot when an agent goes busy" do
    name = AgentFixtures.define(AgentFixtures.unique("e"))
    _ = FakeAgent.start(name)

    {:ok, conn} = Mint.HTTP.connect(:http, "127.0.0.1", port(), mode: :passive)
    {:ok, conn, ref} = Mint.HTTP.request(conn, "GET", "/ui/events", [], nil)

    {conn, first} = next_event(conn, ref, "")
    assert agent(first, name)["state"] == "idle"

    {200, _} = request("POST", "/presence/#{name}", %{state: "busy"})
    {_conn, second} = next_event(conn, ref, "")
    assert agent(second, name)["state"] == "busy"
  end

  defp next_event(conn, ref, buffer) do
    case String.split(buffer, "\n\n", parts: 2) do
      ["data: " <> json, _rest] ->
        {conn, JSON.decode!(json)}

      [": keep-alive", rest] ->
        next_event(conn, ref, rest)

      _ ->
        {:ok, conn, responses} = Mint.HTTP.recv(conn, 0, 5_000)
        data = for {:data, ^ref, d} <- responses, into: "", do: d
        next_event(conn, ref, buffer <> data)
    end
  end

  @tag skip:
         if(System.find_executable("tmux") && System.find_executable("node"),
           do: false,
           else: "needs tmux and node"
         )
  test "clicking an agent switches the tmux session to its window" do
    session = Application.fetch_env!(:aperture, :tmux_session)
    a = AgentFixtures.define(AgentFixtures.unique("f"))
    b = AgentFixtures.define(AgentFixtures.unique("g"))
    {:ok, _} = Launcher.start_agent(a)
    {:ok, _} = Launcher.start_agent(b)
    on_exit(fn -> Launcher.stop_agent(a) && Launcher.stop_agent(b) end)

    assert {200, _} = request("POST", "/agents/#{a}/focus")
    assert Tmux.current_window(session) == a
    assert agent(state(), a)["focused"]

    assert {200, _} = request("POST", "/agents/#{b}/focus")
    assert Tmux.current_window(session) == b
    refute agent(state(), a)["focused"]

    assert {404, _} = request("POST", "/agents/nobody/focus")
  end

  test "other websites and rebinding hosts are refused; this page and plain clients are not" do
    port = port()

    assert {403, _} =
             request("POST", "/agents/glados/restart", nil, [{"origin", "https://evil.example"}])

    assert {403, _} =
             request("POST", "/messages", %{from: "operator", to: "x", body: "y"}, [
               {"origin", "null"}
             ])

    assert {403, _} = request("GET", "/ui/state", nil, [{"host", "evil.example:#{port}"}])

    assert {200, _} = request("GET", "/ui/state", nil, [{"origin", "http://127.0.0.1:#{port}"}])
    assert {200, _} = request("GET", "/ui/state", nil, [{"origin", "http://localhost:#{port}"}])
    assert {200, _} = request("GET", "/ui/state")
  end
end
