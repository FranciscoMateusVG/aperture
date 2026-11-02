defmodule Aperture.Launcher do
  @moduledoc """
  Starts, stops and switches agents, each in its own tmux window, with the
  harness and model chosen for it (see `Aperture.Agents`).

  Claude Code agent, window `<name>`:
    `claude --dangerously-skip-permissions --model M --mcp-config <aperture tools> --settings <busy/idle hooks>
     --append-system-prompt <shared rules + prompt> <kickoff>`; the kickoff has
    it start `aperture-inbox` with the Monitor tool.

  Codex agent, windows `<name>-srv` and `<name>`:
    `codex --dangerously-bypass-approvals-and-sandbox app-server --listen unix://<sock> -c ...` (model, prompt, sandbox,
    the aperture MCP server, personal MCP servers disabled), then the hub's
    bridge binds the agent's thread, then your pane runs
    `codex resume <thread> --remote unix://<sock>`.

  Launches run one at a time. Every generated file lives in `run_dir`.
  """
  use GenServer
  require Logger

  alias Aperture.{Agents, Codex, Tmux}
  alias Aperture.Launcher.Run

  def start_link(_opts), do: GenServer.start_link(__MODULE__, nil, name: __MODULE__)

  @doc "Start every defined agent that is not already running."
  def start_all, do: GenServer.call(__MODULE__, :start_all, :infinity)

  def start_agent(name), do: GenServer.call(__MODULE__, {:start, name}, :infinity)
  def stop_agent(name), do: GenServer.call(__MODULE__, {:stop, name}, :infinity)

  @doc """
  Restarts an agent. Codex normally resumes its owned thread; `fresh_context:
  true` archives the pointer after stopping the bridge, so it creates a new
  thread. Transcripts are never deleted. Claude always launches a new session
  (no --resume, --continue or --session-id); any parked Codex pointer is also
  archived on a fresh reset, so a later harness switch cannot resurrect it.
  """
  def restart_agent(name, opts \\ []),
    do:
      GenServer.call(
        __MODULE__,
        {:restart, name, Keyword.get(opts, :fresh_context, false)},
        :infinity
      )

  @doc "Start several agents at once (a new team), in parallel. Returns %{name => result}."
  def start_many(names), do: GenServer.call(__MODULE__, {:start_many, names}, :infinity)

  @doc "Stop several agents at once, in parallel."
  def stop_many(names), do: GenServer.call(__MODULE__, {:stop_many, names}, :infinity)

  @doc "Save a new harness and/or model for `name` and restart it with them."
  def switch(name, opts) do
    with {:ok, _agent} <- Agents.set(name, opts), do: restart_agent(name)
  end

  # -- GenServer ----------------------------------------------------------------

  @impl true
  def init(nil), do: {:ok, %{}}

  @impl true
  def handle_call(:start_all, _from, state) do
    agents = Agents.list()
    trust(agents)
    results = many(Enum.map(agents, & &1.name), &start/1)
    {:reply, results, state}
  end

  def handle_call({:start_many, names}, _from, state) do
    names |> Enum.map(&Agents.get/1) |> Enum.reject(&is_nil/1) |> trust()
    {:reply, many(names, &start/1), state}
  end

  def handle_call({:stop_many, names}, _from, state), do: {:reply, many(names, &stop/1), state}

  def handle_call({:start, name}, _from, state) do
    trust(List.wrap(Agents.get(name)))
    {:reply, with_agent(name, &start/1), state}
  end

  def handle_call({:stop, name}, _from, state), do: {:reply, with_agent(name, &stop/1), state}

  def handle_call({:restart, name, fresh_context}, _from, state) do
    trust(List.wrap(Agents.get(name)))

    reply =
      with_agent(name, fn agent ->
        stop(agent)
        with :ok <- archive_thread_pointer(agent, fresh_context), do: start(agent)
      end)

    {:reply, reply, state}
  end

  defp archive_thread_pointer(_agent, false), do: :ok

  defp archive_thread_pointer(agent, true) do
    path = Codex.thread_id_path(agent.name)
    suffix = Base.url_encode64(:crypto.strong_rand_bytes(12), padding: false)

    case File.rename(path, path <> ".archived-" <> suffix) do
      :ok -> :ok
      {:error, :enoent} -> :ok
      {:error, reason} -> {:error, {:thread_archive_failed, reason}}
    end
  end

  # different agents touch different windows and files, so they can launch
  # side by side; the session is made once up front so they don't race on it
  defp many(names, fun) do
    Tmux.ensure_session(session())

    names
    |> Task.async_stream(&{&1, with_agent(&1, fun)}, max_concurrency: 8, timeout: :infinity)
    |> Map.new(fn {:ok, result} -> result end)
  end

  # Claude Code asks "Do you trust the files in this folder?" the first time
  # it opens a directory, and a team member has nobody at the keyboard. Mark
  # the folders the lead chose as trusted in Claude's config first: one write
  # for all of them, here in the launcher process, so writes never race.
  defp trust(agents) do
    dirs = for a <- agents, a.team, a.harness == "claude", is_binary(a.cwd), uniq: true, do: a.cwd

    if dirs != [] do
      config = Application.fetch_env!(:aperture, :claude_json)
      script = Path.join(root(), "bin/aperture-trust.mjs")

      case System.cmd(node_bin(), [script, "--config", config | dirs], stderr_to_stdout: true) do
        {_, 0} ->
          :ok

        {out, _} ->
          Logger.warning("launcher: could not pre-trust #{inspect(dirs)}: #{String.trim(out)}")
      end
    end

    :ok
  end

  defp with_agent(name, fun) do
    case Agents.get(name) do
      nil -> {:error, :unknown_agent}
      agent -> fun.(agent)
    end
  end

  # -- start / stop -------------------------------------------------------------

  @doc "Nonblocking lifecycle read; exit receipts contain only code and UTC time."
  def lifecycle(name) do
    case Agents.get(name) do
      nil -> %{state: :offline, exit: nil, run_id: nil}
      agent -> Run.info(run_dir(), primary_window(agent))
    end
  end

  defp primary_window(%{harness: "codex", name: name}), do: name <> "-srv"
  defp primary_window(agent), do: agent.name
  defp process(agent), do: Run.process(run_dir(), session(), primary_window(agent))

  defp start(agent) do
    Tmux.ensure_session(session())
    windows = Tmux.windows(session())
    owned = Enum.filter([agent.name, agent.name <> "-srv"], &(&1 in windows))

    case {owned, process(agent)} do
      {[], :missing} ->
        fresh_start(agent)

      {_, :live} ->
        if agent.harness == "codex" do
          with {:ok, _} <- reattach_codex(agent),
               {:ok, _} <- await_inbox(agent, :already_running) do
            ensure_codex_viewer(agent)
            {:ok, :already_running}
          end
        else
          await_inbox(agent, :already_running)
        end

      {_, state} when state in [:exited, :missing] ->
        # Never destroy another live/unknown companion (e.g. a half-started
        # Codex pair). Only positively exited windows may be recovered.
        if Enum.all?(owned, &(Run.process(run_dir(), session(), &1) == :exited)),
          do: fresh_start(agent),
          else: {:error, {:startup_failed, :liveness_unknown}}

      _ ->
        # Wrapper may be between launch and exec, or from an older hub.
        # Waiting is safe; assuming that an unconnected CLI is dead is not.
        await_inbox(agent, :already_running)
    end
  end

  defp await_inbox(agent, success) do
    timeout = Application.get_env(:aperture, :agent_start_timeout_ms, 30_000)
    deadline = System.monotonic_time(:millisecond) + timeout
    await_inbox(agent, success, deadline)
  end

  defp await_inbox(agent, success, deadline) do
    result =
      cond do
        process(agent) in [:exited, :missing] -> {:error, :process_exited}
        agent.name in Aperture.Router.connected_agents() and process(agent) == :live -> :ready
        System.monotonic_time(:millisecond) >= deadline -> {:error, :inbox_timeout}
        true -> :wait
      end

    case result do
      :ready ->
        Run.state(run_dir(), primary_window(agent), :started)
        {:ok, success}

      {:error, reason} ->
        Run.state(run_dir(), primary_window(agent), :startup_failed)
        Logger.warning("launcher: #{agent.name} startup_failed reason=#{reason}")
        {:error, {:startup_failed, reason}}

      :wait ->
        Process.sleep(25)
        await_inbox(agent, success, deadline)
    end
  end

  # The agent kept running while the hub restarted: its Codex app-server is
  # still up, only the bridge (which lives in the hub) has to come back.
  # Claude agents need nothing: their inbox reconnects by itself.
  defp reattach_codex(agent) do
    case Codex.start_bridge(agent.name, socket_path(agent)) do
      {:ok, _} -> {:ok, :already_running}
      error -> error
    end
  end

  defp fresh_start(agent) do
    # clear leftovers of a half-finished earlier start
    stop(agent)
    File.mkdir_p!(run_dir())
    File.chmod!(run_dir(), 0o700)
    write_prompt(agent)
    do_start(agent.harness, agent)
  end

  # Stop whatever is running for this agent, whichever harness it was on.
  defp stop(agent) do
    _ = Codex.stop_bridge(agent.name)
    Tmux.kill_window(session(), agent.name)
    Tmux.kill_window(session(), agent.name <> "-srv")
    File.rm(socket_path(agent))
    Aperture.Presence.clear(agent.name)
    Run.state(run_dir(), agent.name, :stopped)
    Run.state(run_dir(), agent.name <> "-srv", :stopped)
    :ok
  end

  defp do_start("claude", agent) do
    mcp = path(agent, "mcp.json")

    servers =
      Map.new(agent.mcp, fn {name, spec} ->
        {name, if(spec.env == %{}, do: Map.delete(spec, :env), else: spec)}
      end)

    File.write!(mcp, JSON.encode!(%{mcpServers: Map.put(servers, "aperture", mcp_server(agent))}))

    settings = path(agent, "settings.json")
    File.write!(settings, JSON.encode!(%{hooks: hooks()}))

    command =
      [Application.fetch_env!(:aperture, :claude_bin), "--dangerously-skip-permissions"] ++
        if(agent.model, do: ["--model", agent.model], else: []) ++
        [
          "--mcp-config",
          mcp,
          "--settings",
          settings,
          "--append-system-prompt",
          {:file, path(agent, "prompt.md")},
          claude_kickoff(agent)
        ]

    open_window(agent, agent.name, command)
    await_inbox(agent, :started)
  end

  defp do_start("codex", agent) do
    sock = socket_path(agent)
    File.rm(sock)

    # Codex 0.160 accepts the bypass at the CLI root, not after app-server.
    # Keep explicit server config too: the bridge reinforces it per RPC.
    server =
      [
        Application.fetch_env!(:aperture, :codex_bin),
        "--dangerously-bypass-approvals-and-sandbox",
        "app-server",
        "--listen",
        "unix://" <> sock
      ] ++
        codex_overrides(agent)

    open_window(agent, agent.name <> "-srv", server)

    with :ok <- wait_until(fn -> File.exists?(sock) end, "codex app-server socket"),
         {:ok, _} <- Codex.start_bridge(agent.name, sock),
         :ok <-
           wait_until(fn -> agent.name in Aperture.Router.connected_agents() end, "bridge bind"),
         {:ok, thread} <- File.read(Codex.thread_id_path(agent.name)) do
      open_codex_viewer(agent, String.trim(thread))
      await_inbox(agent, :started)
    else
      {:error, reason} ->
        Logger.warning("launcher: #{agent.name} (codex) startup_failed: #{inspect(reason)}")
        Run.state(run_dir(), primary_window(agent), :startup_failed)
        {:error, {:startup_failed, reason}}
    end
  end

  defp ensure_codex_viewer(agent) do
    if Run.process(run_dir(), session(), agent.name) in [:missing, :exited] do
      Tmux.kill_window(session(), agent.name)

      case File.read(Codex.thread_id_path(agent.name)) do
        {:ok, thread} -> open_codex_viewer(agent, String.trim(thread))
        _ -> :ok
      end
    end
  end

  defp open_codex_viewer(agent, thread) do
    # Permissions belong to the app-server, never to remote resume.
    pane =
      [
        Application.fetch_env!(:aperture, :codex_bin),
        "resume",
        thread,
        "--remote",
        "unix://" <> socket_path(agent),
        "-c",
        "check_for_update_on_startup=false"
      ] ++
        if(agent.model, do: ["-m", agent.model], else: [])

    open_window(agent, agent.name, pane)
  end

  # -- pieces -------------------------------------------------------------------

  defp codex_overrides(agent) do
    aperture = mcp_server(agent)

    settings =
      [
        {"approval_policy", "never"},
        {"sandbox_mode", "danger-full-access"},
        {"check_for_update_on_startup", false},
        {"model_instructions_file", path(agent, "prompt.md")},
        {"mcp_servers.aperture.command", aperture.command},
        {"mcp_servers.aperture.args", aperture.args}
      ] ++
        if(agent.model, do: [{"model", agent.model}], else: []) ++
        Enum.flat_map(Enum.sort(agent.mcp), fn {name, spec} ->
          [{"mcp_servers.#{name}.command", spec.command}, {"mcp_servers.#{name}.args", spec.args}] ++
            if(spec.env == %{}, do: [], else: [{"mcp_servers.#{name}.env", {:table, spec.env}}])
        end) ++
        for(
          name <- disabled_mcp_servers(),
          not Map.has_key?(agent.mcp, name),
          do: {"mcp_servers.#{name}.enabled", false}
        )

    Enum.flat_map(settings, fn {key, value} -> ["-c", "#{key}=#{toml(value)}"] end)
  end

  # JSON strings, booleans and string arrays are valid TOML values; tables are
  # written as TOML inline tables (keys are validated identifiers).
  defp toml({:table, map}),
    do:
      "{" <>
        Enum.map_join(Enum.sort(map), ", ", fn {k, v} -> "#{k} = #{JSON.encode!(v)}" end) <> "}"

  defp toml(value), do: JSON.encode!(value)

  # Codex agents share ~/.codex, so every MCP server defined there would load
  # into them too. Disable exactly those. Never name a server that is not
  # defined: `-c mcp_servers.X.enabled=false` on its own creates a half
  # entry with no transport, and Codex refuses to start ("invalid transport").
  @doc false
  def disabled_mcp_servers do
    case File.read(Application.get_env(:aperture, :codex_config, "")) do
      {:ok, toml} ->
        for [_, _, name] <- Regex.scan(~r/^\s*\[mcp_servers\.("?)([A-Za-z0-9_-]+)\1\]/m, toml),
            name != "aperture",
            uniq: true,
            do: name

      _ ->
        []
    end
  end

  defp mcp_server(agent) do
    %{
      command: node_bin(),
      args:
        [
          Path.join(root(), "bin/aperture-mcp.mjs"),
          "--agent",
          agent.name,
          "--hub",
          hub_http()
        ] ++ if(Aperture.Teams.lead?(agent.name), do: ["--lead", "true"], else: [])
    }
  end

  defp hooks do
    state = fn s ->
      [
        %{
          hooks: [
            %{
              type: "command",
              command: "#{node_bin()} #{Path.join(root(), "bin/aperture-state.mjs")} #{s}"
            }
          ]
        }
      ]
    end

    %{UserPromptSubmit: state.("busy"), PreToolUse: state.("busy"), Stop: state.("idle")}
  end

  defp claude_kickoff(agent) do
    inbox =
      "#{node_bin()} #{Path.join(root(), "bin/aperture-inbox.mjs")} --agent #{agent.name} --url #{hub_ws()}"

    "Session start. Start your inbox now: use the Monitor tool to run `#{inbox}` and keep it " <>
      "running for the whole session. Each line it prints is a message from another agent. " <>
      "Then wait for messages."
  end

  defp write_prompt(%{team: team} = agent) when is_binary(team) do
    File.write!(path(agent, "prompt.md"), Aperture.TeamPrompt.render(agent.name))
  end

  defp write_prompt(agent) do
    shared = Path.join(root(), "prompts/_shared.md")
    parts = for p <- [shared, agent.prompt_path], File.exists?(p), do: File.read!(p)
    header = "You are #{agent.display} (`#{agent.name}`). Role: #{agent.role}\n\n"
    File.write!(path(agent, "prompt.md"), header <> Enum.join(parts, "\n\n"))
  end

  # Each window runs a generated script, so nothing is re-quoted through tmux.
  defp open_window(agent, window, command) do
    script = Path.join(run_dir(), "#{window}.sh")

    env =
      Map.merge(
        %{
          "APERTURE_AGENT" => agent.name,
          "APERTURE_HUB_HTTP" => hub_http(),
          "APERTURE_HUB_WS" => hub_ws()
        },
        Application.get_env(:aperture, :launch_env, %{})
      )

    body =
      [
        "#!/usr/bin/env bash",
        for({k, v} <- Enum.sort(env), do: "export #{k}=#{sh(v)}"),
        "cd #{sh(agent.cwd || root())}",
        "exec " <> Enum.map_join(command, " ", &arg/1)
      ]
      |> List.flatten()
      |> Enum.join("\n")

    File.write!(script, body <> "\n")
    File.chmod!(script, 0o700)

    receipt = Run.prepare(run_dir(), window)
    wrapper = Path.join(receipt, "wrapper.sh")
    exit_file = Path.join(receipt, "exit.json")

    File.write!(wrapper, """
    #!/usr/bin/env bash
    umask 077
    bash #{sh(script)}
    code=$?
    at=$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)
    printf '{"code":%d,"at":"%s"}\\n' "$code" "$at" > #{sh(exit_file <> ".tmp")}
    mv #{sh(exit_file <> ".tmp")} #{sh(exit_file)}
    printf '\\n[#{window} exited (code %s), press enter]\\n' "$code"
    read -r _
    """)

    File.chmod!(wrapper, 0o700)
    Tmux.new_window(session(), window, "bash #{sh(wrapper)}")
  end

  defp arg({:file, file}), do: ~s["$(cat #{sh(file)})"]
  defp arg(value), do: sh(value)

  defp sh(value), do: "'" <> String.replace(to_string(value), "'", "'\\''") <> "'"

  defp wait_until(fun, what) do
    deadline =
      System.monotonic_time(:millisecond) + Application.get_env(:aperture, :codex_wait_ms, 30_000)

    do_wait(fun, what, deadline)
  end

  defp do_wait(fun, what, deadline) do
    cond do
      fun.() ->
        :ok

      System.monotonic_time(:millisecond) > deadline ->
        {:error, {:timeout, what}}

      true ->
        Process.sleep(100)
        do_wait(fun, what, deadline)
    end
  end

  defp socket_path(agent), do: path(agent, "sock")
  defp path(agent, suffix), do: Path.join(run_dir(), "#{agent.name}.#{suffix}")
  defp session, do: Application.fetch_env!(:aperture, :tmux_session)
  defp run_dir, do: Application.fetch_env!(:aperture, :run_dir)
  defp root, do: Application.fetch_env!(:aperture, :root)
  defp port, do: Application.fetch_env!(:aperture, :port)
  defp hub_http, do: "http://127.0.0.1:#{port()}"
  defp hub_ws, do: "ws://127.0.0.1:#{port()}/inbox"
  defp node_bin, do: System.find_executable("node") || "node"
end
