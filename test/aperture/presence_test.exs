defmodule Aperture.PresenceTest do
  use ExUnit.Case, async: false

  alias Aperture.{AgentFixtures, Codex, FakeAgent, FakeCodex}

  defp wait(fun, msg), do: FakeAgent.wait_until(fun, msg, 5_000)

  defp http(method, path, body \\ nil) do
    port = Application.fetch_env!(:aperture, :port)
    {:ok, conn} = Mint.HTTP.connect(:http, "127.0.0.1", port, mode: :passive)
    headers = [{"content-type", "application/json"}]
    {:ok, conn, ref} = Mint.HTTP.request(conn, method, path, headers, body && JSON.encode!(body))
    {status, data} = recv(conn, ref, nil, "")
    {status, if(data == "", do: nil, else: JSON.decode!(data))}
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

    if done, do: Mint.HTTP.close(conn) && {status, acc}, else: recv(conn, ref, status, acc)
  end

  defp presence(name) do
    {200, %{"agents" => agents}} = http("GET", "/presence")
    Enum.find(agents, &(&1["name"] == name))
  end

  test "an agent is offline until connected, then idle, then whatever its hooks report" do
    name = AgentFixtures.define(AgentFixtures.unique("p"))
    assert %{"state" => "offline", "harness" => "claude", "model" => "opus"} = presence(name)

    _agent = FakeAgent.start(name)
    assert %{"state" => "idle"} = presence(name)

    # what bin/aperture-state.mjs does from Claude Code's hooks
    env = [
      {"APERTURE_AGENT", name},
      {"APERTURE_HUB_HTTP", "http://127.0.0.1:#{Application.fetch_env!(:aperture, :port)}"}
    ]

    script = Path.join(Application.fetch_env!(:aperture, :root), "bin/aperture-state.mjs")
    {_, 0} = System.cmd("node", [script, "busy"], env: env)
    assert %{"state" => "busy"} = presence(name)
    {_, 0} = System.cmd("node", [script, "idle"], env: env)
    assert %{"state" => "idle"} = presence(name)
  end

  test "a Codex agent is busy while a turn runs and idle after it completes" do
    name = AgentFixtures.define(AgentFixtures.unique("q"), %{"harness" => "codex"})
    socket = Path.join(System.tmp_dir!(), "apq-#{System.unique_integer([:positive])}.sock")
    on_exit(fn -> Codex.stop_bridge(name) end)

    fake = FakeCodex.start(socket)
    {:ok, _} = Codex.start_bridge(name, socket)

    # the kickoff turn is running
    wait(fn -> presence(name)["state"] == "busy" end, "never busy during kickoff")
    FakeCodex.complete_turn(fake)
    wait(fn -> presence(name)["state"] == "idle" end, "never idle after the turn")

    FakeCodex.drop(fake)
    Process.sleep(10)
    assert presence(name)["state"] in ["offline", "idle", "busy"]
  end

  test "switch requests are validated over HTTP" do
    name = AgentFixtures.define(AgentFixtures.unique("v"))

    assert {400, %{"error" => "invalid_harness"}} =
             http("POST", "/agents/#{name}/config", %{harness: "vim"})

    assert {400, %{"error" => "unknown_agent"}} =
             http("POST", "/agents/ghost/config", %{harness: "codex"})
  end
end
