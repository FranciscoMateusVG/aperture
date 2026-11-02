defmodule Aperture.Codex.Bridge do
  @moduledoc """
  Delivers one Codex agent's messages through its `codex app-server`.

  Connects to the app-server's unix socket (WebSocket, JSON-RPC 2.0), then:

    1. `initialize`, then the `initialized` notification
    2. resumes the thread this agent owns (`<run_dir>/<agent>.thread-id`), or,
       if it has none or it is gone, `thread/start`s a fresh one, records it
       there, and sends the kickoff turn. It never resumes a thread it did not
       create: Codex agents share the operator's `~/.codex`, so other threads
       may be the operator's own sessions.
    3. the same file lets the operator's tmux pane run
       `codex resume <id> --remote unix://<socket>` on that thread
    4. registers as the agent's live connection and injects every unacked
       message: `turn/steer` (with `expectedTurnId`) when a turn is in
       progress, `turn/start` otherwise

  The agent's configured model is passed on `thread/start`, `thread/resume`
  and every `turn/start`: a resumed thread otherwise keeps the model it was
  created with, whatever the app-server was launched with.

  The active turn id is learned from `turn/started`, from `turn/start` /
  `turn/steer` results, and from the resumed thread's turns; it is cleared on
  `turn/completed|failed|aborted` and when the thread goes idle.

  A message is acked in the store only when its inject call succeeds. A failed
  steer forgets the turn id and retries as `turn/start` (which Codex accepts
  mid-turn); anything still unacked when the socket drops is re-injected after
  the bridge reconnects and rebinds. The tmux pane is never typed into.
  """
  use GenServer
  require Logger

  alias Aperture.{Router, Store, WS}

  @control_timeout 10_000
  @turn_timeout 30 * 60_000
  @max_inject_attempts 3

  def start_link(opts) do
    agent = Keyword.fetch!(opts, :agent)

    GenServer.start_link(__MODULE__, opts,
      name: {:via, Registry, {Aperture.Codex.Bridges, agent}}
    )
  end

  @doc "The kickoff turn for a brand-new Codex session. Static apart from the validated name."
  def kickoff_text(agent) do
    "Session start. You are #{agent}, an Aperture agent. Messages from other agents " <>
      "arrive as turns starting with [aperture]. Reply and send with the aperture " <>
      "send_message tool; list_agents shows who is online. Wait for messages."
  end

  # -- GenServer ----------------------------------------------------------------

  @impl true
  def init(opts) do
    Process.flag(:trap_exit, true)
    send(self(), :connect)

    {:ok,
     %{
       agent: Keyword.fetch!(opts, :agent),
       model: configured_model(Keyword.fetch!(opts, :agent)),
       socket: Keyword.fetch!(opts, :socket),
       ws: nil,
       attempt: 0,
       offline_logged: false,
       phase: :disconnected,
       next_id: 1,
       pending: %{},
       thread_id: nil,
       active_turn: nil,
       queue: [],
       inflight: %{},
       failures: %{},
       retry_armed: false
     }}
  end

  @impl true
  def handle_info(message, state) do
    {:noreply, new_state} = handle(message, state)
    report_presence(state, new_state)
    {:noreply, new_state}
  end

  # Codex is busy while a turn is in progress.
  defp report_presence(old, new) do
    busy? = fn s -> s.active_turn != nil end

    cond do
      new.phase == :bound and (old.phase != :bound or busy?.(old) != busy?.(new)) ->
        Aperture.Presence.set(new.agent, if(busy?.(new), do: :busy, else: :idle))

      old.phase == :bound and new.phase != :bound ->
        Aperture.Presence.clear(new.agent)

      true ->
        :ok
    end
  end

  defp handle(:connect, state) do
    case WS.connect({:unix, state.socket}, "/") do
      {:ok, ws} ->
        Logger.info("codex bridge #{state.agent}: connected to #{state.socket}")

        %{state | ws: ws, attempt: 0, offline_logged: false, phase: :initializing}
        |> rpc(
          "initialize",
          %{clientInfo: %{name: "aperture", title: "Aperture", version: "4.0.0-dev"}},
          :initialize
        )
        |> noreply()

      {:error, reason} ->
        state =
          if state.offline_logged do
            state
          else
            Logger.info(
              "codex bridge #{state.agent}: app-server not reachable (#{inspect(reason)}), retrying"
            )

            %{state | offline_logged: true}
          end

        {:noreply, schedule_reconnect(state)}
    end
  end

  defp handle({:push, msg}, %{phase: :bound} = state) do
    if Map.has_key?(state.inflight, msg.id) or Enum.any?(state.queue, &(&1.id == msg.id)) do
      {:noreply, state}
    else
      noreply(flush(%{state | queue: state.queue ++ [msg]}))
    end
  end

  # Not bound yet: the message is in the store and will be replayed on bind.
  defp handle({:push, _msg}, state), do: {:noreply, state}

  defp handle(:flush, state), do: noreply(flush(%{state | retry_armed: false}))

  defp handle({:rpc_timeout, id}, state) do
    case Map.pop(state.pending, id) do
      {nil, _} ->
        {:noreply, state}

      {{method, meta, _timer}, pending} ->
        noreply(on_error(%{state | pending: pending}, method, meta, "timeout"))
    end
  end

  # Someone else (e.g. a Claude inbox) took this agent name. Step aside.
  defp handle(:replaced, state) do
    Logger.warning("codex bridge #{state.agent}: another connection took this agent name")
    Router.unregister(state.agent)
    {:noreply, state}
  end

  defp handle(message, %{ws: ws} = state) when ws != nil do
    case WS.handle_message(ws, message) do
      :unknown ->
        {:noreply, state}

      {:ok, ws, texts} ->
        noreply(Enum.reduce(texts, %{state | ws: ws}, &handle_text/2))

      {:closed, _ws, reason} ->
        Logger.info(
          "codex bridge #{state.agent}: app-server connection closed (#{inspect(reason)})"
        )

        noreply(disconnect(state))
    end
  end

  defp handle(_message, state), do: {:noreply, state}

  @impl true
  def terminate(_reason, state), do: WS.close(state.ws)

  # -- JSON-RPC -----------------------------------------------------------------

  defp handle_text(text, state) do
    case JSON.decode(text) do
      {:ok, %{"id" => id, "method" => _}} when not is_nil(id) ->
        # A server -> client request (e.g. an approval). The operator's TUI
        # answers those; the bridge only delivers messages.
        state

      {:ok, %{"id" => id} = resp} ->
        case Map.pop(state.pending, id) do
          {nil, _} ->
            state

          {{method, meta, timer}, pending} ->
            Process.cancel_timer(timer)
            state = %{state | pending: pending}

            case resp do
              %{"error" => error} -> on_error(state, method, meta, error)
              _ -> on_result(state, meta, Map.get(resp, "result"))
            end
        end

      {:ok, %{"method" => method} = note} ->
        on_notification(state, method, Map.get(note, "params") || %{})

      _ ->
        state
    end
  end

  defp rpc(%{ws: nil} = state, _method, _params, _meta), do: state

  defp rpc(state, method, params, meta) do
    id = state.next_id
    frame = JSON.encode!(%{jsonrpc: "2.0", id: id, method: method, params: params})

    case WS.send_text(state.ws, frame) do
      {:ok, ws} ->
        timeout =
          if String.starts_with?(method, "turn/"), do: @turn_timeout, else: @control_timeout

        timer = Process.send_after(self(), {:rpc_timeout, id}, timeout)

        %{
          state
          | ws: ws,
            next_id: id + 1,
            pending: Map.put(state.pending, id, {method, meta, timer})
        }

      {:error, _ws, _reason} ->
        disconnect(state)
    end
  end

  defp notify(%{ws: nil} = state, _method, _params), do: state

  defp notify(state, method, params) do
    case WS.send_text(state.ws, JSON.encode!(%{jsonrpc: "2.0", method: method, params: params})) do
      {:ok, ws} -> %{state | ws: ws}
      {:error, _ws, _} -> disconnect(state)
    end
  end

  # -- handshake and binding ------------------------------------------------------

  defp on_result(state, :initialize, _result) do
    state
    |> notify("initialized", %{})
    |> Map.put(:phase, :binding)
    |> resume_or_start()
  end

  defp on_result(state, {:resume, id}, result) do
    bind(%{state | active_turn: in_progress_turn(result)}, id)
  end

  defp on_result(state, :thread_start, result) do
    case thread_id_from(result) do
      nil ->
        Logger.warning(
          "codex bridge #{state.agent}: thread/start returned no id: #{inspect(result)}"
        )

        disconnect(state)

      id ->
        state
        |> Map.put(:thread_id, id)
        |> rpc(
          "turn/start",
          turn_params(state, id, kickoff_text(state.agent)),
          {:inject, [], "turn/start"}
        )
        |> bind(id)
    end
  end

  defp on_result(state, {:inject, ids, method}, result) do
    Enum.each(ids, &Store.ack(&1, state.agent))

    %{
      state
      | inflight: Map.drop(state.inflight, ids),
        failures: Map.drop(state.failures, ids),
        active_turn: turn_from_result(method, result, state.active_turn)
    }
  end

  # turn/start answers with the Turn; only trust it while it is in progress
  # (the response can arrive after the turn already completed).
  defp turn_from_result("turn/start", %{"turn" => %{"id" => id} = turn}, current)
       when is_binary(id) do
    if turn["status"] in [nil, "inProgress"], do: id, else: current
  end

  defp turn_from_result("turn/steer", %{"turnId" => id}, _current) when is_binary(id), do: id
  defp turn_from_result(_method, _result, current), do: current

  defp in_progress_turn(%{"thread" => %{"turns" => turns}}) when is_list(turns) do
    turns
    |> Enum.reverse()
    |> Enum.find_value(fn
      %{"id" => id, "status" => "inProgress"} when is_binary(id) -> id
      _ -> nil
    end)
  end

  defp in_progress_turn(_), do: nil

  defp resume_or_start(state) do
    case owned_thread(state.agent) do
      nil -> rpc(state, "thread/start", thread_params(state), :thread_start)
      id -> rpc(state, "thread/resume", thread_params(state, %{threadId: id}), {:resume, id})
    end
  end

  defp owned_thread(agent) do
    with {:ok, contents} <- File.read(Aperture.Codex.thread_id_path(agent)),
         id = String.trim(contents),
         true <- id =~ ~r/^[A-Za-z0-9-]{1,128}$/ do
      id
    else
      _ -> nil
    end
  end

  defp thread_id_from(%{"thread" => %{"id" => id}}) when is_binary(id), do: id
  defp thread_id_from(%{"threadId" => id}) when is_binary(id), do: id
  defp thread_id_from(%{"id" => id}) when is_binary(id), do: id
  defp thread_id_from(_), do: nil

  defp bind(%{ws: nil} = state, _id), do: state

  defp bind(state, thread_id) do
    if thread_id =~ ~r/^[A-Za-z0-9-]{1,128}$/ do
      publish_thread_id(state.agent, thread_id)
      :ok = Router.register(state.agent)
      Logger.info("codex bridge #{state.agent}: bound to thread #{thread_id}")

      replay = Enum.reject(Store.unacked(state.agent), &Map.has_key?(state.inflight, &1.id))
      flush(%{state | thread_id: thread_id, phase: :bound, queue: replay})
    else
      Logger.warning("codex bridge #{state.agent}: refusing malformed thread id")
      disconnect(state)
    end
  end

  defp publish_thread_id(agent, thread_id) do
    path = Aperture.Codex.thread_id_path(agent)
    File.mkdir_p!(Path.dirname(path))
    tmp = path <> ".tmp"
    File.write!(tmp, thread_id <> "\n")
    File.chmod!(tmp, 0o600)
    File.rename!(tmp, path)
  end

  # -- delivery -----------------------------------------------------------------

  defp flush(%{phase: :bound, queue: [_ | _], retry_armed: false} = state) do
    msgs = state.queue
    ids = Enum.map(msgs, & &1.id)
    text = Enum.map_join(msgs, "\n\n", &format/1)
    images = for m <- msgs, a <- Map.get(m, :attachments, []), do: a.path

    {method, params} =
      case state.active_turn do
        nil ->
          {"turn/start", turn_params(state, state.thread_id, text, images)}

        # turn/steer takes no model: it joins the turn already running
        turn_id ->
          {"turn/steer",
           %{threadId: state.thread_id, input: input(text, images), expectedTurnId: turn_id}}
      end

    %{state | queue: [], inflight: Map.merge(state.inflight, Map.new(msgs, &{&1.id, &1}))}
    |> rpc(method, params, {:inject, ids, method})
  end

  defp flush(state), do: state

  defp format(msg), do: "[aperture] message ##{msg.id} from #{msg.from}: #{msg.body}"

  # A resumed thread may have persisted restrictive settings. Never inherit
  # them, even when no model is configured. Codex 0.160 uses `sandbox` for
  # thread RPCs but `sandboxPolicy` (tagged object) for turn/start.
  defp thread_params(state, params \\ %{}),
    do:
      with_model(
        Map.merge(params, %{approvalPolicy: "never", sandbox: "danger-full-access"}),
        state
      )

  defp turn_params(state, thread_id, text, images \\ []),
    do:
      with_model(
        %{
          threadId: thread_id,
          input: input(text, images),
          approvalPolicy: "never",
          sandboxPolicy: %{type: "dangerFullAccess"}
        },
        state
      )

  # images attached in the operator's chat: their paths are in the text too,
  # and Codex also gets them as image inputs
  defp input(text, images),
    do: [%{type: "text", text: text} | Enum.map(images, &%{type: "localImage", path: &1})]

  defp with_model(params, %{model: nil}), do: params
  defp with_model(params, %{model: model}), do: Map.put(params, :model, model)

  # nil for a bridge without a Codex agent definition (Codex picks its default)
  defp configured_model(agent) do
    case Aperture.Agents.get(agent) do
      %{harness: "codex", model: model} -> model
      _ -> nil
    end
  end

  # -- errors -------------------------------------------------------------------

  defp on_error(state, method, {:inject, ids, _}, error) do
    Logger.warning("codex bridge #{state.agent}: #{method} failed: #{inspect(error)}")
    {msgs, inflight} = Map.split(state.inflight, ids)
    failures = Enum.reduce(ids, state.failures, &Map.update(&2, &1, 1, fn n -> n + 1 end))

    # Give up for now on messages that keep failing; they stay unacked in the
    # store and are re-injected after the next reconnect.
    retry =
      msgs
      |> Map.values()
      |> Enum.filter(&(failures[&1.id] < @max_inject_attempts))
      |> Enum.sort_by(& &1.id)

    # A failed steer means our turn id is stale (or the turn just ended):
    # forget it, so the retry is a turn/start, which Codex accepts mid-turn.
    state = %{
      state
      | inflight: inflight,
        failures: failures,
        active_turn: if(method == "turn/steer", do: nil, else: state.active_turn),
        queue: retry ++ state.queue
    }

    if retry != [] and state.ws != nil and not state.retry_armed do
      Process.send_after(self(), :flush, Application.get_env(:aperture, :codex_retry_ms, 1_000))
      %{state | retry_armed: true}
    else
      state
    end
  end

  # Our recorded thread is gone (deleted, or from another Codex home): start a
  # fresh one. A timeout is not proof it is gone, so that reconnects instead.
  defp on_error(state, "thread/resume", {:resume, id}, error) when error != "timeout" do
    Logger.warning(
      "codex bridge #{state.agent}: cannot resume thread #{id} (#{inspect(error)}), starting a new one"
    )

    File.rm(Aperture.Codex.thread_id_path(state.agent))
    rpc(state, "thread/start", thread_params(state), :thread_start)
  end

  # initialize / thread/* failed or timed out: start over.
  defp on_error(state, method, _meta, error) do
    Logger.warning("codex bridge #{state.agent}: #{method} failed: #{inspect(error)}")
    disconnect(state)
  end

  # -- turn state -----------------------------------------------------------------

  defp on_notification(state, method, params) do
    tid = params["threadId"] || params["thread_id"]

    cond do
      is_binary(tid) and state.thread_id != nil and tid != state.thread_id ->
        state

      method == "turn/started" ->
        case get_in(params, ["turn", "id"]) || params["turnId"] do
          id when is_binary(id) -> %{state | active_turn: id}
          _ -> state
        end

      method in ["turn/completed", "turn/failed", "turn/aborted"] ->
        ended = get_in(params, ["turn", "id"]) || params["turnId"]

        if ended == nil or ended == state.active_turn,
          do: flush(%{state | active_turn: nil}),
          else: state

      method == "thread/status/changed" ->
        case status_of(params["status"]) do
          :idle -> flush(%{state | active_turn: nil})
          _ -> state
        end

      true ->
        state
    end
  end

  defp status_of(%{"type" => type}), do: status_of(type)

  defp status_of(status) when is_binary(status) do
    cond do
      status =~ ~r/^(idle|ready|completed)$/i -> :idle
      status =~ ~r/^(active|busy|running|generating|in_?progress|turn.*)$/i -> :busy
      true -> :unknown
    end
  end

  defp status_of(_), do: :unknown

  # -- connection lifecycle -------------------------------------------------------

  defp disconnect(state) do
    if state.phase == :bound, do: Router.unregister(state.agent)
    Enum.each(state.pending, fn {_id, {_, _, timer}} -> Process.cancel_timer(timer) end)
    WS.close(state.ws)

    schedule_reconnect(%{
      state
      | ws: nil,
        phase: :disconnected,
        pending: %{},
        thread_id: nil,
        active_turn: nil,
        queue: [],
        inflight: %{},
        failures: %{},
        retry_armed: false
    })
  end

  defp schedule_reconnect(state) do
    backoff =
      Application.get_env(:aperture, :codex_backoff_ms, [500, 1_000, 2_000, 5_000, 10_000])

    Process.send_after(self(), :connect, Enum.at(backoff, state.attempt, List.last(backoff)))
    %{state | attempt: state.attempt + 1}
  end

  defp noreply(state), do: {:noreply, state}
end
