defmodule Aperture.FakeCodex do
  @moduledoc """
  A scripted stand-in for `codex app-server --listen unix://<socket>`.

  JSON-RPC 2.0 over WebSocket on a unix socket, with the subset of methods the
  bridge uses. Shapes follow the codex-cli 0.158 app-server schema:

    * `turn/start` -> `{turn: {id, status}}`; starts a turn and emits
      `turn/started`, or, mid-turn, is accepted into the active turn
    * `turn/steer` requires `expectedTurnId` matching the active turn,
      returns `{turnId}`
    * `thread/resume` returns the thread with its turns (an active one is
      `inProgress`), or an error for a thread it does not have
    * `complete_turn/1` emits `turn/completed`

  Test controls: `drop/1`, `fail_next/2`, `change_turn_silently/1`.
  It records every call so tests can assert on what Codex would have received.
  """

  defmodule Socket do
    @moduledoc false
    @behaviour WebSock

    @impl true
    def init(server) do
      pid = self()
      Agent.update(server, &%{&1 | socket: pid})
      {:ok, server}
    end

    @impl true
    def handle_in({text, [opcode: :text]}, server) do
      case JSON.decode!(text) do
        %{"id" => id, "method" => method} = req ->
          params = req["params"] || %{}
          Agent.update(server, &%{&1 | calls: &1.calls ++ [{method, params}]})
          reply(server, id, method, params)

        %{"method" => method} ->
          Agent.update(server, &%{&1 | calls: &1.calls ++ [{method, :notification}]})
          {:ok, server}
      end
    end

    @impl true
    def handle_info({:notify, method, params}, server),
      do: {:push, note(method, params), server}

    def handle_info(:drop, server), do: {:stop, :normal, server}

    defp reply(server, id, method, params) do
      {result, notes} =
        Agent.get_and_update(server, fn s ->
          if s.fail_next == method do
            {{{:error, "scripted failure"}, []}, %{s | fail_next: nil}}
          else
            handle(method, params, s)
          end
        end)

      frame =
        case result do
          {:ok, value} -> %{jsonrpc: "2.0", id: id, result: value}
          {:error, msg} -> %{jsonrpc: "2.0", id: id, error: %{code: -32600, message: msg}}
        end

      {:push, [{:text, JSON.encode!(frame)} | Enum.map(notes, fn {m, p} -> note(m, p) end)],
       server}
    end

    defp note(method, params),
      do: {:text, JSON.encode!(%{jsonrpc: "2.0", method: method, params: params})}

    defp ok(value, s, notes \\ []), do: {{{:ok, value}, notes}, s}
    defp err(msg, s), do: {{{:error, msg}, []}, s}

    defp handle("initialize", _p, s), do: ok(%{userAgent: "fake-codex"}, s)

    defp handle("thread/list", _p, s),
      do: ok(%{data: Enum.map(Enum.reverse(s.threads), &%{id: &1})}, s)

    defp handle("thread/start", _p, s) do
      id = "thread-#{length(s.threads) + 1}"
      ok(%{thread: %{id: id, turns: []}}, %{s | threads: [id | s.threads]})
    end

    defp handle("thread/resume", %{"threadId" => id}, s) do
      if id in s.threads, do: resume(id, s), else: err("thread not found: #{id}", s)
    end

    # Mid-turn, Codex 0.158 accepts turn/start into the active turn.
    defp handle("turn/start", _p, %{active_turn: turn} = s) when turn != nil,
      do: ok(%{turn: %{id: turn, status: "inProgress"}}, s)

    defp handle("turn/start", p, s) do
      turn = "turn-#{s.turn_seq + 1}"

      started =
        {"turn/started", %{threadId: p["threadId"], turn: %{id: turn, status: "inProgress"}}}

      ok(
        %{turn: %{id: turn, status: "inProgress"}},
        %{s | active_turn: turn, turn_seq: s.turn_seq + 1},
        [
          started
        ]
      )
    end

    defp handle("turn/steer", p, s) do
      case p["expectedTurnId"] do
        nil -> err("Invalid request: missing field `expectedTurnId`", s)
        _ when s.active_turn == nil -> err("no active turn", s)
        turn when turn != s.active_turn -> err("expectedTurnId does not match the active turn", s)
        turn -> ok(%{turnId: turn}, s)
      end
    end

    defp handle(method, _p, s), do: err("unknown method #{method}", s)

    defp resume(id, s) do
      turns =
        if s.active_turn,
          do: [%{id: "old-turn", status: "completed"}, %{id: s.active_turn, status: "inProgress"}],
          else: [%{id: "old-turn", status: "completed"}]

      ok(%{thread: %{id: id, turns: turns}}, s)
    end
  end

  defmodule Plug do
    @moduledoc false
    def init(server), do: server

    def call(conn, server) do
      conn
      |> WebSockAdapter.upgrade(Aperture.FakeCodex.Socket, server, timeout: 60_000)
      |> Elixir.Plug.Conn.halt()
    end
  end

  # -- test API -----------------------------------------------------------------

  @doc """
  Start a fake app-server on `socket`.
  Options: `threads: [ids]` (existing threads), `active_turn: id` (a turn in progress).
  """
  def start(socket, opts \\ []) do
    File.rm(socket)

    {:ok, server} =
      Agent.start_link(fn ->
        %{
          socket: nil,
          calls: [],
          threads: Keyword.get(opts, :threads, []),
          active_turn: Keyword.get(opts, :active_turn),
          turn_seq: 0,
          fail_next: nil
        }
      end)

    {:ok, bandit} =
      Bandit.start_link(
        plug: {__MODULE__.Plug, server},
        ip: {:local, socket},
        port: 0,
        startup_log: false
      )

    %{server: server, bandit: bandit, socket: socket}
  end

  def calls(%{server: s}), do: Agent.get(s, & &1.calls)

  @doc "Every turn/start and turn/steer, in order, as {method, text, params}."
  def injects(fake) do
    for {method, %{"input" => [%{"text" => text} | _]} = params} <- calls(fake),
        method in ["turn/start", "turn/steer"],
        do: {method, text, params}
  end

  @doc "Every turn/start and turn/steer, in order, as {method, text}."
  def injected(fake), do: Enum.map(injects(fake), fn {m, t, _} -> {m, t} end)

  def count(fake, method), do: Enum.count(calls(fake), &(elem(&1, 0) == method))

  def active_turn(%{server: s}), do: Agent.get(s, & &1.active_turn)

  def fail_next(%{server: s}, method), do: Agent.update(s, &%{&1 | fail_next: method})

  @doc "End the active turn and tell the client, like Codex does."
  def complete_turn(%{server: s}) do
    {socket, turn} =
      Agent.get_and_update(s, &{{&1.socket, &1.active_turn}, %{&1 | active_turn: nil}})

    send(socket, {:notify, "turn/completed", %{turn: %{id: turn, status: "completed"}}})
  end

  @doc "Replace the active turn without telling the client (a missed notification)."
  def change_turn_silently(%{server: s}),
    do: Agent.update(s, &%{&1 | active_turn: "turn-silent", turn_seq: &1.turn_seq + 1})

  @doc "Close the bridge's connection (the server keeps listening)."
  def drop(%{server: s}), do: send(Agent.get(s, & &1.socket), :drop)
end
