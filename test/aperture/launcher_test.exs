defmodule Aperture.LauncherTest do
  @moduledoc """
  The launcher with fake `claude` / `codex` binaries (test/support/bin) in a
  private tmux server. fake-claude records its arguments and runs the real
  aperture-inbox; fake-codex records its arguments and idles, while the test
  plays the app-server with FakeCodex on the socket the launcher chose.
  """
  use ExUnit.Case, async: false

  alias Aperture.{AgentFixtures, Codex, FakeAgent, FakeCodex, Launcher, Tmux}

  @moduletag skip:
               if(System.find_executable("tmux") && System.find_executable("node"),
                 do: false,
                 else: "needs tmux and node"
               )

  setup_all do
    on_exit(fn -> Tmux.kill_session(Application.fetch_env!(:aperture, :tmux_session)) end)
  end

  setup do
    File.write!(Application.fetch_env!(:aperture, :codex_config), """
    model = "whatever"

    [mcp_servers.personal-notes]
    command = "notes"

    [mcp_servers."quoted_one"]
    command = "q"

    [mcp_servers.memory]
    command = "old-memory"
    """)

    :ok
  end

  defp session, do: Application.fetch_env!(:aperture, :tmux_session)
  defp log_dir, do: Application.fetch_env!(:aperture, :launch_env)["FAKE_LOG_DIR"]
  defp wait(fun, msg, timeout \\ 8_000), do: FakeAgent.wait_until(fun, msg, timeout)

  defp args(file) do
    path = Path.join(log_dir(), file)
    # non-empty: the shell creates the file before printf fills it
    wait(fn -> match?({:ok, %{size: n}} when n > 0, File.stat(path)) end, "#{file} never written")
    path |> File.read!() |> String.split(<<0>>, trim: true)
  end

  defp value_after(args, flag), do: Enum.at(args, Enum.find_index(args, &(&1 == flag)) + 1)

  defp overrides(args) do
    args
    |> Enum.chunk_every(2, 1, :discard)
    |> Enum.filter(fn [flag, _] -> flag == "-c" end)
    |> Map.new(fn [_, kv] ->
      [k, v] = String.split(kv, "=", parts: 2)

      case JSON.decode(v) do
        {:ok, decoded} -> {k, decoded}
        {:error, _} -> {k, {:raw, v}}
      end
    end)
  end

  # Start a Codex agent: the launcher waits for the app-server socket, so play
  # the app-server once fake-codex has told us where it should listen.
  defp start_codex(name, launch \\ &Launcher.start_agent/1, fake_opts \\ []) do
    task = Task.async(fn -> launch.(name) end)
    server = args("codex-app-server-#{name}.args")
    "unix://" <> socket = value_after(server, "--listen")
    fake = FakeCodex.start(socket, fake_opts)
    assert {:ok, :started} = Task.await(task, 15_000)
    assert ["--dangerously-bypass-approvals-and-sandbox", "app-server" | _] = server
    # Remote resume is only a viewer; permissions belong to the app-server.
    # Codex 0.160 rejects permission overrides here before the TUI can attach.
    resume = args("codex-resume-#{name}.args")
    refute "--dangerously-bypass-approvals-and-sandbox" in resume
    refute "--full-auto" in resume
    refute "--ask-for-approval" in resume
    refute "--sandbox" in resume
    refute Map.has_key?(overrides(resume), "approval_policy")
    refute Map.has_key?(overrides(resume), "sandbox_mode")
    {server, socket, fake}
  end

  test "a Claude Code agent starts with its model, tools, hooks and prompt, and comes online" do
    name =
      AgentFixtures.define(AgentFixtures.unique("c"), %{
        "model" => "sonnet",
        "mcp" => %{
          "memory" => %{
            "command" => "~/bin/mem",
            "args" => ["--palace", "~/palace"],
            "env" => %{"MEM_MODE" => "ro"}
          }
        }
      })

    assert {:ok, :started} = Launcher.start_agent(name)
    assert name in Tmux.windows(session())

    a = args("claude-#{name}.args")
    assert value_after(a, "--model") == "sonnet"
    assert "--dangerously-skip-permissions" in a

    mcp = a |> value_after("--mcp-config") |> File.read!() |> JSON.decode!()
    assert ["--agent", ^name | _] = Enum.drop(mcp["mcpServers"]["aperture"]["args"], 1)

    # the agent's own extra MCP servers (e.g. mempalace), with ~ expanded
    home = System.user_home!()

    assert mcp["mcpServers"]["memory"] == %{
             "command" => Path.join(home, "bin/mem"),
             "args" => ["--palace", Path.join(home, "palace")],
             "env" => %{"MEM_MODE" => "ro"}
           }

    hooks = a |> value_after("--settings") |> File.read!() |> JSON.decode!()
    assert hooks["hooks"]["Stop"] |> inspect() =~ "aperture-state.mjs idle"
    assert hooks["hooks"]["UserPromptSubmit"] |> inspect() =~ "aperture-state.mjs busy"

    prompt = value_after(a, "--append-system-prompt")
    assert prompt =~ "Test prompt for #{name}"
    assert prompt =~ "# Aperture"
    assert List.last(a) =~ "Monitor tool"
    assert List.last(a) =~ "aperture-inbox.mjs --agent #{name}"

    # fake-claude runs the real inbox, so a message shows up in its pane
    wait(fn -> name in Aperture.Router.connected_agents() end, "agent never came online")
    sender = FakeAgent.start(AgentFixtures.unique("s"))
    FakeAgent.send_message(sender, name, "hello launched agent")
    wait(fn -> Tmux.capture(session(), name) =~ "hello launched agent" end, "message not in pane")
  end

  test "a Codex agent: app-server with its settings, bridge bound, pane resumes the thread" do
    name =
      AgentFixtures.define(AgentFixtures.unique("x"), %{
        "harness" => "codex",
        "model" => "gpt-6-astra",
        "mcp" => %{
          "memory" => %{
            "command" => "/opt/mem",
            "args" => ["--ro"],
            "env" => %{"MEM_MODE" => "ro"}
          }
        }
      })

    {server, socket, _fake} = start_codex(name)
    c = overrides(server)

    assert c["model"] == "gpt-6-astra"
    assert c["approval_policy"] == "never"
    assert c["sandbox_mode"] == "danger-full-access"
    assert c["check_for_update_on_startup"] == false
    assert ["--agent", ^name | _] = Enum.drop(c["mcp_servers.aperture.args"], 1)
    assert File.read!(c["model_instructions_file"]) =~ "Test prompt for #{name}"

    # personal MCP servers off: exactly the ones ~/.codex/config.toml defines
    # (naming an undefined one makes Codex refuse to start: "invalid transport")
    assert c["mcp_servers.personal-notes.enabled"] == false
    assert c["mcp_servers.quoted_one.enabled"] == false
    refute Map.has_key?(c, "mcp_servers.aperture.enabled")

    # the agent's own servers are given to it, and never disabled even when
    # ~/.codex/config.toml defines one with the same name
    assert c["mcp_servers.memory.command"] == "/opt/mem"
    assert c["mcp_servers.memory.args"] == ["--ro"]
    assert c["mcp_servers.memory.env"] == {:raw, ~s({MEM_MODE = "ro"})}

    assert Enum.sort(for {"mcp_servers." <> rest, false} <- c, do: rest) ==
             ["personal-notes.enabled", "quoted_one.enabled"]

    assert name in Aperture.Router.connected_agents()
    thread = Codex.thread_id_path(name) |> File.read!() |> String.trim()

    assert ["resume", ^thread, "--remote", "unix://" <> ^socket | _] =
             args("codex-resume-#{name}.args")

    assert Enum.sort(Tmux.windows(session()) |> Enum.filter(&String.starts_with?(&1, name))) ==
             [name, name <> "-srv"]
  end

  test "changing a Codex agent's model reaches its resumed thread, not just the app-server" do
    name =
      AgentFixtures.define(AgentFixtures.unique("m"), %{
        "harness" => "codex",
        "model" => "gpt-6-astra"
      })

    {_server, _socket, first} = start_codex(name)
    thread = Codex.thread_id_path(name) |> File.read!() |> String.trim()

    assert [
             {"thread/start",
              %{
                "model" => "gpt-6-astra",
                "approvalPolicy" => "never",
                "sandbox" => "danger-full-access"
              }}
           ] =
             for({"thread/start", _} = c <- FakeCodex.calls(first), do: c)

    # Codex keeps a resumed thread on the model it was created with unless
    # thread/resume and turn/start say otherwise (found live in M3)
    # the first launch's pane must have written its args before they are cleared
    _ = args("codex-resume-#{name}.args")

    for f <- ["codex-app-server-#{name}.args", "codex-resume-#{name}.args"],
        do: File.rm!(Path.join(log_dir(), f))

    {server, _socket, fake} =
      start_codex(name, &Launcher.switch(&1, model: "gpt-5.6-sol"), threads: [thread])

    assert overrides(server)["model"] == "gpt-5.6-sol"

    assert [
             {"thread/resume",
              %{
                "threadId" => ^thread,
                "model" => "gpt-5.6-sol",
                "approvalPolicy" => "never",
                "sandbox" => "danger-full-access"
              }}
           ] =
             for({"thread/resume", _} = c <- FakeCodex.calls(fake), do: c)

    assert value_after(args("codex-resume-#{name}.args"), "-m") == "gpt-5.6-sol"

    sender = FakeAgent.start(AgentFixtures.unique("snd"))
    FakeAgent.send_message(sender, name, "which model are you?")
    wait(fn -> FakeCodex.injects(fake) != [] end, "never injected")

    assert [
             {"turn/start", _text,
              %{
                "model" => "gpt-5.6-sol",
                "approvalPolicy" => "never",
                "sandboxPolicy" => %{"type" => "dangerFullAccess"}
              }}
           ] = FakeCodex.injects(fake)
  end

  test "switching an agent's harness restarts it on the other one and keeps its Codex thread" do
    name =
      AgentFixtures.define(AgentFixtures.unique("w"), %{
        "harness" => "codex",
        "model" => "gpt-6-astra"
      })

    {_server, _socket, _fake} = start_codex(name)
    thread_file = Codex.thread_id_path(name)
    thread = File.read!(thread_file)

    assert {:ok, :started} = Launcher.switch(name, harness: "claude", model: "opus")

    refute (name <> "-srv") in Tmux.windows(session())
    assert value_after(args("claude-#{name}.args"), "--model") == "opus"
    assert "--dangerously-skip-permissions" in args("claude-#{name}.args")
    wait(fn -> name in Aperture.Router.connected_agents() end, "not online on claude")
    assert Codex.stop_bridge(name) == {:error, :not_found}
    assert File.read!(thread_file) == thread
    assert %{harness: "claude", model: "opus"} = Aperture.Agents.get(name)
  end

  test "Restart keeps a coordinator's Claude permission bypass" do
    name = AgentFixtures.define(AgentFixtures.unique("restart"))
    assert {:ok, :started} = Launcher.start_agent(name)
    assert "--dangerously-skip-permissions" in args("claude-#{name}.args")
    File.rm!(Path.join(log_dir(), "claude-#{name}.args"))
    assert {:ok, :started} = Launcher.restart_agent(name)
    assert "--dangerously-skip-permissions" in args("claude-#{name}.args")
  end

  test "fresh context archives the Codex pointer and starts a new thread; normal Restart resumes" do
    name = AgentFixtures.define(AgentFixtures.unique("fresh"), %{"harness" => "codex"})
    {_server, _socket, _first} = start_codex(name)
    path = Codex.thread_id_path(name)
    original = File.read!(path)
    thread = String.trim(original)

    for f <- ["codex-app-server-#{name}.args", "codex-resume-#{name}.args"],
        do: File.rm!(Path.join(log_dir(), f))

    {_server, _socket, normal} = start_codex(name, &Launcher.restart_agent/1, threads: [thread])
    assert File.read!(path) == original

    assert Enum.any?(
             FakeCodex.calls(normal),
             &match?({"thread/resume", %{"threadId" => ^thread}}, &1)
           )

    refute Enum.any?(FakeCodex.calls(normal), &match?({"thread/start", _}, &1))

    for f <- ["codex-app-server-#{name}.args", "codex-resume-#{name}.args"],
        do: File.rm!(Path.join(log_dir(), f))

    {_server, _socket, fresh} =
      start_codex(
        name,
        &Launcher.restart_agent(&1, fresh_context: true),
        threads: [thread]
      )

    refute File.read!(path) == original
    refute Enum.any?(FakeCodex.calls(fresh), &match?({"thread/resume", _}, &1))
    assert Enum.any?(FakeCodex.calls(fresh), &match?({"thread/start", _}, &1))
    [archived] = Path.wildcard(path <> ".archived-*")
    assert File.read!(archived) == original
    assert ["resume", new_thread | _] = args("codex-resume-#{name}.args")
    assert new_thread == String.trim(File.read!(path))
    refute new_thread == thread
  end

  test "Claude fresh context never supplies a session reuse flag and archives a parked Codex pointer" do
    name = AgentFixtures.define(AgentFixtures.unique("freshc"))
    assert {:ok, :started} = Launcher.start_agent(name)
    path = Codex.thread_id_path(name)
    File.write!(path, "parked-codex-thread\n")
    _ = args("claude-#{name}.args")

    for opts <- [[], [fresh_context: true]] do
      File.rm!(Path.join(log_dir(), "claude-#{name}.args"))
      assert {:ok, :started} = Launcher.restart_agent(name, opts)
      argv = args("claude-#{name}.args")
      for flag <- ["--resume", "--continue", "--session-id", "-r", "-c"], do: refute(flag in argv)
      if opts == [], do: assert(File.read!(path) == "parked-codex-thread\n")
    end

    refute File.exists?(path)
    [archived] = Path.wildcard(path <> ".archived-*")
    assert File.read!(archived) == "parked-codex-thread\n"
  end

  test "starting a running agent is a no-op; stopping takes it offline" do
    name = AgentFixtures.define(AgentFixtures.unique("s"))
    assert {:ok, :started} = Launcher.start_agent(name)
    wait(fn -> name in Aperture.Router.connected_agents() end, "never online")

    assert {:ok, :already_running} = Launcher.start_agent(name)
    assert Enum.count(Tmux.windows(session()), &(&1 == name)) == 1

    assert :ok = Launcher.stop_agent(name)
    refute name in Tmux.windows(session())
    wait(fn -> name not in Aperture.Router.connected_agents() end, "still online after stop")
  end

  test "after a hub restart, a running Codex agent gets its bridge back without relaunching" do
    name = AgentFixtures.define(AgentFixtures.unique("r"), %{"harness" => "codex"})
    {_server, _socket, fake} = start_codex(name)
    server_args = Path.join(log_dir(), "codex-app-server-#{name}.args")
    File.rm!(server_args)

    # the hub went away: its bridge is gone, the app-server and panes are not
    :ok = Codex.stop_bridge(name)
    wait(fn -> name not in Aperture.Router.connected_agents() end, "still online")

    assert {:ok, :already_running} = Launcher.start_agent(name)
    wait(fn -> name in Aperture.Router.connected_agents() end, "bridge not back")
    refute File.exists?(server_args), "app-server was relaunched"
    assert FakeCodex.count(fake, "thread/start") == 1
  end

  defp mode(name, value), do: File.write!(Path.join(log_dir(), "claude-#{name}.mode"), value)

  test "an exited CLI records metadata and Start recovers its retained tmux window" do
    name = AgentFixtures.define(AgentFixtures.unique("exit"))
    File.mkdir_p!(log_dir())
    mode(name, "exit")
    assert {:error, {:startup_failed, :process_exited}} = Launcher.start_agent(name)
    assert name in Tmux.windows(session())

    assert %{state: :startup_failed, exit: %{"code" => 23, "at" => at}, run_id: old_run} =
             Launcher.lifecycle(name)

    assert {:ok, _, _} = DateTime.from_iso8601(at)
    assert Aperture.Presence.state(name) == :startup_failed
    mode(name, "")
    assert {:ok, :started} = Launcher.start_agent(name)
    assert name in Aperture.Router.connected_agents()
    assert %{state: :started, exit: nil, run_id: new_run} = Launcher.lifecycle(name)
    refute old_run == new_run

    receipt =
      Path.join([
        Application.fetch_env!(:aperture, :run_dir),
        "lifecycle",
        name,
        old_run,
        "exit.json"
      ])

    assert %{"code" => 23, "at" => ^at} = JSON.decode!(File.read!(receipt))
    assert Map.keys(JSON.decode!(File.read!(receipt))) |> Enum.sort() == ["at", "code"]
  end

  test "started waits for the inbox and concurrent Start never replaces a mid-start agent" do
    name = AgentFixtures.define(AgentFixtures.unique("slow"))
    File.mkdir_p!(log_dir())
    mode(name, "delayed")
    first = Task.async(fn -> Launcher.start_agent(name) end)
    _ = args("claude-#{name}.args")
    assert Task.yield(first, 50) == nil
    assert %{state: :starting} = Launcher.lifecycle(name)
    second = Task.async(fn -> Launcher.start_agent(name) end)
    assert {:ok, :started} = Task.await(first)
    assert {:ok, :already_running} = Task.await(second)
    assert Enum.count(Tmux.windows(session()), &(&1 == name)) == 1
  end

  test "inbox timeout is startup_failed, but a live unconnected CLI is never killed by Start" do
    old = Application.get_env(:aperture, :agent_start_timeout_ms)
    Application.put_env(:aperture, :agent_start_timeout_ms, 300)

    on_exit(fn ->
      if old,
        do: Application.put_env(:aperture, :agent_start_timeout_ms, old),
        else: Application.delete_env(:aperture, :agent_start_timeout_ms)
    end)

    name = AgentFixtures.define(AgentFixtures.unique("silent"))
    File.mkdir_p!(log_dir())
    mode(name, "no-inbox")
    assert {:error, {:startup_failed, :inbox_timeout}} = Launcher.start_agent(name)
    assert %{state: :startup_failed, run_id: run_id, exit: nil} = Launcher.lifecycle(name)
    File.rm!(Path.join(log_dir(), "claude-#{name}.args"))
    assert {:error, {:startup_failed, :inbox_timeout}} = Launcher.start_agent(name)
    assert %{run_id: ^run_id, exit: nil} = Launcher.lifecycle(name)
    refute File.exists?(Path.join(log_dir(), "claude-#{name}.args"))
    assert Aperture.Presence.state(name) == :startup_failed
    Launcher.stop_agent(name)
  end

  test "Start restores a missing Codex viewer without replacing its live app-server" do
    name = AgentFixtures.define(AgentFixtures.unique("viewer"), %{"harness" => "codex"})
    {_server, _socket, fake} = start_codex(name)
    %{run_id: run} = Launcher.lifecycle(name)
    Tmux.kill_window(session(), name)
    assert {:ok, :already_running} = Launcher.start_agent(name)
    assert name in Tmux.windows(session())
    assert %{run_id: ^run} = Launcher.lifecycle(name)
    assert FakeCodex.count(fake, "thread/start") == 1
  end

  test "Stop clears an unconnected failed startup without leaving starting presence" do
    old = Application.get_env(:aperture, :agent_start_timeout_ms)
    Application.put_env(:aperture, :agent_start_timeout_ms, 100)

    on_exit(fn ->
      if old,
        do: Application.put_env(:aperture, :agent_start_timeout_ms, old),
        else: Application.delete_env(:aperture, :agent_start_timeout_ms)
    end)

    name = AgentFixtures.define(AgentFixtures.unique("stopfailed"))
    File.mkdir_p!(log_dir())
    mode(name, "no-inbox")
    assert {:error, {:startup_failed, :inbox_timeout}} = Launcher.start_agent(name)
    assert :ok = Launcher.stop_agent(name)
    assert Aperture.Presence.state(name) == :offline
  end

  test "start_all waits for stalled inboxes in parallel, not one timeout per agent" do
    keys = [:agents_dir, :teams_path, :agent_start_timeout_ms]
    previous = Map.new(keys, &{&1, Application.get_env(:aperture, &1)})
    dir = Path.join(System.tmp_dir!(), "aperture-boot-#{System.unique_integer([:positive])}")
    Application.put_env(:aperture, :agents_dir, dir)
    Application.put_env(:aperture, :teams_path, Path.join(dir, "no-teams"))
    Application.put_env(:aperture, :agent_start_timeout_ms, 1_200)
    names = for n <- ["first", "second"], do: AgentFixtures.define(n)
    File.mkdir_p!(log_dir())
    Enum.each(names, &mode(&1, "no-inbox"))

    on_exit(fn ->
      Enum.each(names, &Launcher.stop_agent/1)

      Enum.each(previous, fn {key, value} ->
        if value,
          do: Application.put_env(:aperture, key, value),
          else: Application.delete_env(:aperture, key)
      end)

      File.rm_rf!(dir)
    end)

    started = System.monotonic_time(:millisecond)
    result = Launcher.start_all()
    elapsed = System.monotonic_time(:millisecond) - started
    assert Map.keys(result) |> Enum.sort() == names

    assert Enum.all?(result, fn {_, status} ->
             status == {:error, {:startup_failed, :inbox_timeout}}
           end)

    assert elapsed < 2_000, "two 1200ms deadlines were serialized: #{elapsed}ms"
  end
end
