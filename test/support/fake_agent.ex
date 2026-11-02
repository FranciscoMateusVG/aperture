defmodule Aperture.FakeAgent do
  @moduledoc """
  A scripted stand-in for a real agent client (`aperture-inbox`).

  Speaks the real WebSocket protocol against the real hub, so integration
  tests exercise the actual delivery path in milliseconds, with no LLM.

  Behaves like the real client must:
    * reconnects with backoff after any drop, then says `hello` again
    * exits (normally) when the hub says it was replaced (close code 4001)
    * acks every message it receives
    * hands each message id to the "agent" exactly once (dedupe by id)

  Test controls: `send_message/3`, `drop/1`, `received/1`, `wait_for/3`.
  """
  use GenServer

  alias Aperture.WS

  defstruct [:name, :pid, :log]

  @backoff_ms [50, 100, 200, 400, 800]

  # -- test API ---------------------------------------------------------------

  def start(name) do
    {:ok, log} =
      Agent.start_link(fn ->
        %{received: [], seen: MapSet.new(), frames: 0, welcomes: 0, sent: []}
      end)

    {:ok, pid} = GenServer.start_link(__MODULE__, %{name: name, log: log})
    fake = %__MODULE__{name: name, pid: pid, log: log}
    wait_until(fn -> welcomes(fake) >= 1 end, "#{name} never got welcome")
    fake
  end

  @doc "Send a message as this agent, the way an agent's send_message tool would."
  def send_message(%__MODULE__{pid: pid}, to, body), do: GenServer.cast(pid, {:send, to, body})

  @doc "Close the socket from the client side; the client will reconnect."
  def drop(%__MODULE__{pid: pid}), do: GenServer.cast(pid, :drop)

  @doc "Bodies the agent has seen, in delivery order, each id once."
  def received(%__MODULE__{log: log}),
    do: Agent.get(log, &Enum.reverse(&1.received)) |> Enum.map(& &1["body"])

  def welcomes(%__MODULE__{log: log}), do: Agent.get(log, & &1.welcomes)

  @doc "Raw message frames received, duplicates included."
  def frames(%__MODULE__{log: log}), do: Agent.get(log, & &1.frames)

  def sent(%__MODULE__{log: log}), do: Agent.get(log, &Enum.reverse(&1.sent))

  def wait_for(fake, count, timeout \\ 5_000) do
    wait_until(
      fn -> length(received(fake)) >= count end,
      "#{fake.name} got #{length(received(fake))}/#{count}",
      timeout
    )

    received(fake)
  end

  def wait_until(fun, msg, timeout \\ 5_000) do
    deadline = System.monotonic_time(:millisecond) + timeout
    do_wait(fun, msg, deadline)
  end

  defp do_wait(fun, msg, deadline) do
    cond do
      fun.() ->
        :ok

      System.monotonic_time(:millisecond) > deadline ->
        raise ExUnit.AssertionError, message: "timeout: " <> msg

      true ->
        Process.sleep(10)
        do_wait(fun, msg, deadline)
    end
  end

  # -- GenServer ----------------------------------------------------------------

  @impl true
  def init(%{name: name, log: log}) do
    send(self(), :connect)
    {:ok, %{name: name, log: log, ws: nil, attempt: 0}}
  end

  @impl true
  def handle_info(:connect, state) do
    port = Application.fetch_env!(:aperture, :port)

    case WS.connect({:tcp, "127.0.0.1", port}, "/inbox") do
      {:ok, ws} ->
        {:noreply, push(%{state | ws: ws, attempt: 0}, %{type: "hello", agent: state.name})}

      {:error, _} ->
        {:noreply, reconnect(state)}
    end
  end

  def handle_info(message, %{ws: ws} = state) when ws != nil do
    case WS.handle_message(ws, message) do
      :unknown ->
        {:noreply, state}

      {:ok, ws, texts} ->
        {:noreply, Enum.reduce(texts, %{state | ws: ws}, &handle_text/2)}

      {:closed, _ws, {:remote_close, 4001, _}} ->
        {:stop, :normal, %{state | ws: nil}}

      {:closed, _ws, _reason} ->
        {:noreply, reconnect(%{state | ws: nil})}
    end
  end

  def handle_info(_message, state), do: {:noreply, state}

  @impl true
  def handle_cast({:send, to, body}, state),
    do: {:noreply, push(state, %{type: "send", to: to, body: body})}

  def handle_cast(:drop, state) do
    WS.close(state.ws)
    {:noreply, reconnect(%{state | ws: nil})}
  end

  defp handle_text(text, state) do
    case JSON.decode!(text) do
      %{"type" => "message", "id" => id} = msg ->
        Agent.update(state.log, fn log ->
          log = %{log | frames: log.frames + 1}

          if MapSet.member?(log.seen, id),
            do: log,
            else: %{log | received: [msg | log.received], seen: MapSet.put(log.seen, id)}
        end)

        push(state, %{type: "ack", id: id})

      %{"type" => "welcome"} ->
        Agent.update(state.log, &%{&1 | welcomes: &1.welcomes + 1})
        state

      %{"type" => "sent"} = sent ->
        Agent.update(state.log, &%{&1 | sent: [sent | &1.sent]})
        state

      _other ->
        state
    end
  end

  defp push(%{ws: nil} = state, _frame), do: state

  defp push(state, frame) do
    case WS.send_text(state.ws, JSON.encode!(frame)) do
      {:ok, ws} -> %{state | ws: ws}
      {:error, _ws, _} -> reconnect(%{state | ws: nil})
    end
  end

  defp reconnect(state) do
    delay = Enum.at(@backoff_ms, state.attempt, List.last(@backoff_ms))
    Process.send_after(self(), :connect, delay)
    %{state | attempt: state.attempt + 1}
  end
end
