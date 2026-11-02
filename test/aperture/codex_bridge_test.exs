defmodule Aperture.CodexBridgeTest do
  @moduledoc """
  The Codex bridge against a fake `codex app-server`, with the real hub.
  """
  use ExUnit.Case, async: false

  alias Aperture.{Codex, FakeAgent, FakeCodex}

  setup context do
    n = System.unique_integer([:positive])
    socket = Path.join(System.tmp_dir!(), "apc-#{n}.sock")
    codex = "codex-#{context.line}-#{n}"

    on_exit(fn ->
      Codex.stop_bridge(codex)
      File.rm(socket)
    end)

    {:ok, socket: socket, codex: codex, a: "a-#{context.line}-#{n}"}
  end

  defp wait(fun, msg, timeout \\ 5_000), do: FakeAgent.wait_until(fun, msg, timeout)

  defp injected_text?(fake, fragment),
    do: Enum.any?(FakeCodex.injected(fake), fn {_, text} -> text =~ fragment end)

  defp assert_full_permissions(fake) do
    calls =
      for {method, params} <- FakeCodex.calls(fake),
          method in ["thread/start", "thread/resume", "turn/start"],
          do: {method, params}

    assert calls != []

    for {method, params} <- calls do
      assert params["approvalPolicy"] == "never"

      if method == "turn/start" do
        assert params["sandboxPolicy"] == %{"type" => "dangerFullAccess"}
        refute Map.has_key?(params, "sandbox")
      else
        assert params["sandbox"] == "danger-full-access"
      end
    end
  end

  # Record `id` as the thread this agent owns, as a previous run would have.
  defp own(codex, id) do
    path = Codex.thread_id_path(codex)
    File.mkdir_p!(Path.dirname(path))
    File.write!(path, id <> "\n")
  end

  defp start_bound(socket, codex, opts \\ []) do
    if owned = opts[:owned], do: own(codex, owned)
    fake = FakeCodex.start(socket, Keyword.delete(opts, :owned))
    {:ok, _} = Codex.start_bridge(codex, socket)
    wait(fn -> codex in Aperture.Router.connected_agents() end, "bridge never bound")
    fake
  end

  test "fresh app-server: one thread, one kickoff, thread id published", %{
    socket: socket,
    codex: codex
  } do
    fake = start_bound(socket, codex)

    assert FakeCodex.count(fake, "thread/start") == 1
    assert_full_permissions(fake)
    assert [{"turn/start", kickoff}] = FakeCodex.injected(fake)
    assert kickoff =~ "You are #{codex}"
    assert File.read!(Codex.thread_id_path(codex)) == "thread-1\n"
  end

  defp injects_for(fake, fragment),
    do: Enum.filter(FakeCodex.injects(fake), fn {_, text, _} -> text =~ fragment end)

  test "busy Codex gets turn/steer with the active turn id; idle Codex gets turn/start",
       %{socket: socket, codex: codex, a: a} do
    fake = start_bound(socket, codex)
    fa = FakeAgent.start(a)

    # the kickoff turn is still running
    FakeAgent.send_message(fa, codex, "while busy")
    wait(fn -> injected_text?(fake, "while busy") end, "not injected")

    assert [{"turn/steer", text, params}] = injects_for(fake, "while busy")
    assert params["expectedTurnId"] == FakeCodex.active_turn(fake)
    assert text == "[aperture] message ##{hd(FakeAgent.sent(fa))["id"]} from #{a}: while busy"

    FakeCodex.complete_turn(fake)
    Process.sleep(50)
    FakeAgent.send_message(fa, codex, "while idle")
    wait(fn -> injected_text?(fake, "while idle") end, "not injected")
    assert [{"turn/start", _, _}] = injects_for(fake, "while idle")
    assert_full_permissions(fake)
  end

  test "resuming mid-turn: the bridge learns the in-progress turn and steers into it",
       %{socket: socket, codex: codex, a: a} do
    fake = start_bound(socket, codex, threads: ["t-1"], owned: "t-1", active_turn: "turn-9")
    fa = FakeAgent.start(a)

    FakeAgent.send_message(fa, codex, "join the running turn")
    wait(fn -> injected_text?(fake, "join the running turn") end, "not injected")

    assert [{"turn/steer", _, %{"expectedTurnId" => "turn-9"}}] =
             injects_for(fake, "join the running turn")

    assert_full_permissions(fake)
  end

  test "a missed turn notification: stale steer falls back to turn/start",
       %{socket: socket, codex: codex, a: a} do
    fake = start_bound(socket, codex)
    fa = FakeAgent.start(a)
    FakeCodex.change_turn_silently(fake)

    FakeAgent.send_message(fa, codex, "stale turn")
    wait(fn -> length(injects_for(fake, "stale turn")) >= 2 end, "no fallback")

    assert [{"turn/steer", _, %{"expectedTurnId" => "turn-1"}}, {"turn/start", _, _}] =
             injects_for(fake, "stale turn")

    assert_full_permissions(fake)
  end

  test "messages sent before Codex is up are injected once it binds", %{
    socket: socket,
    codex: codex,
    a: a
  } do
    fa = FakeAgent.start(a)
    FakeAgent.send_message(fa, codex, "queued for codex")
    wait(fn -> length(FakeAgent.sent(fa)) == 1 end, "send not stored")
    assert [%{"status" => "queued"}] = FakeAgent.sent(fa)

    fake = start_bound(socket, codex)
    wait(fn -> injected_text?(fake, "queued for codex") end, "queued message never injected")
  end

  test "owned thread: resumed after a restart, no kickoff", %{socket: socket, codex: codex} do
    fake = start_bound(socket, codex, threads: ["t-own"], owned: "t-own")

    assert FakeCodex.count(fake, "thread/start") == 0

    assert {"thread/resume",
            %{
              "threadId" => "t-own",
              "approvalPolicy" => "never",
              "sandbox" => "danger-full-access"
            }} in FakeCodex.calls(fake)

    assert_full_permissions(fake)
    assert FakeCodex.injected(fake) == []
  end

  test "never resumes a thread it does not own (e.g. the operator's own session)",
       %{socket: socket, codex: codex} do
    fake = start_bound(socket, codex, threads: ["personal-session"])

    refute Enum.any?(FakeCodex.calls(fake), &match?({"thread/resume", _}, &1))
    assert FakeCodex.count(fake, "thread/start") == 1
    assert [{"turn/start", _kickoff}] = FakeCodex.injected(fake)
    assert File.read!(Codex.thread_id_path(codex)) == "thread-2\n"
  end

  test "owned thread is gone: starts a fresh one and records it", %{socket: socket, codex: codex} do
    fake = start_bound(socket, codex, owned: "vanished")

    assert {"thread/resume",
            %{
              "threadId" => "vanished",
              "approvalPolicy" => "never",
              "sandbox" => "danger-full-access"
            }} in FakeCodex.calls(fake)

    assert FakeCodex.count(fake, "thread/start") == 1
    assert [{"turn/start", _kickoff}] = FakeCodex.injected(fake)
    assert File.read!(Codex.thread_id_path(codex)) == "thread-1\n"
    assert_full_permissions(fake)
  end

  test "connection drop: rebinds the same thread, no second kickoff, acked messages not resent",
       %{socket: socket, codex: codex, a: a} do
    fake = start_bound(socket, codex)
    fa = FakeAgent.start(a)

    FakeAgent.send_message(fa, codex, "before drop")
    wait(fn -> injected_text?(fake, "before drop") end, "not injected")
    Process.sleep(50)

    FakeCodex.drop(fake)
    wait(fn -> FakeCodex.count(fake, "thread/resume") == 1 end, "bridge did not rebind")
    wait(fn -> codex in Aperture.Router.connected_agents() end, "bridge not registered again")

    FakeAgent.send_message(fa, codex, "after drop")
    wait(fn -> injected_text?(fake, "after drop") end, "not injected after rebind")

    assert FakeCodex.count(fake, "thread/start") == 1
    assert Enum.count(FakeCodex.injected(fake), fn {_, t} -> t =~ "You are" end) == 1
    assert Enum.count(FakeCodex.injected(fake), fn {_, t} -> t =~ "before drop" end) == 1
    # the kickoff turn is still running after the rebind: steered, first try
    assert [{"turn/steer", _, _}] = injects_for(fake, "after drop")
    assert_full_permissions(fake)
  end

  test "a failed inject is retried with the other method", %{socket: socket, codex: codex, a: a} do
    fake = start_bound(socket, codex)
    fa = FakeAgent.start(a)

    FakeCodex.fail_next(fake, "turn/steer")
    FakeAgent.send_message(fa, codex, "retry me")

    wait(
      fn -> Enum.count(FakeCodex.injected(fake), fn {_, t} -> t =~ "retry me" end) >= 2 end,
      "no retry"
    )

    assert [{"turn/steer", _}, {"turn/start", _} | _] =
             Enum.filter(FakeCodex.injected(fake), fn {_, t} -> t =~ "retry me" end)
  end

  test "bridges are started over HTTP and refuse unsafe agent names", %{
    socket: socket,
    codex: codex
  } do
    _fake = FakeCodex.start(socket)
    port = Application.fetch_env!(:aperture, :port)

    post = fn body ->
      {:ok, conn} = Mint.HTTP.connect(:http, "127.0.0.1", port, mode: :passive)
      headers = [{"content-type", "application/json"}]

      {:ok, conn, ref} =
        Mint.HTTP.request(conn, "POST", "/codex/bridges", headers, JSON.encode!(body))

      {:ok, conn, responses} = Mint.HTTP.recv(conn, 0, 5_000)
      Mint.HTTP.close(conn)

      Enum.find_value(responses, fn
        {:status, ^ref, status} -> status
        _ -> nil
      end)
    end

    assert post.(%{agent: "../etc", socket: socket}) == 400
    assert post.(%{agent: codex, socket: socket}) == 200
    wait(fn -> codex in Aperture.Router.connected_agents() end, "bridge never bound")
  end
end
