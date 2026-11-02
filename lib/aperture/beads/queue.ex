defmodule Aperture.Beads.Queue do
  @moduledoc """
  One bd subprocess at a time, including polls and recovery reads.
  Calls have an absolute budget below the MCP client's timeout; queued work
  checks its age before starting. Caller liveness alone is NOT an HTTP cancel.
  """
  use GenServer
  require Logger

  @port_key :aperture_bd_active_port
  @deadline_key :aperture_bd_deadline

  def start_link(_), do: GenServer.start_link(__MODULE__, nil, name: __MODULE__)

  def run(op, fun) do
    budget = budget_ms()
    GenServer.call(__MODULE__, {op, fun, now(), budget}, budget)
  catch
    :exit, _ -> uncertain(op, "bd call deadline or connection lost")
  end

  # Hard caps cannot be raised beyond the MCP budget by application config.
  @doc false
  def budget_ms, do: bounded(:bd_total_timeout_ms, 45_000)
  @doc false
  def bounded(key, max), do: min(max, max(1, Application.get_env(:aperture, key, max)))

  # Port ownership and this ledger live in the same process. Record immediately
  # after spawn, and clear ONLY after consuming its exit-status, not in `after`.
  @doc false
  def track(port), do: Process.put(@port_key, port)
  @doc false
  def confirm_exit(port) do
    if Process.get(@port_key) == port, do: Process.delete(@port_key)
    :ok
  end

  @doc false
  def deadline, do: Process.get(@deadline_key, now() + 43_000)

  # Nonblocking health read: a GenServer.call would wait behind the stuck CLI.
  def health do
    case :ets.lookup(__MODULE__, :status) do
      [{:status, status}] -> status
      _ -> "unavailable"
    end
  rescue
    ArgumentError -> "unavailable"
  end

  defp now, do: System.monotonic_time(:millisecond)
  defp status(value), do: :ets.insert(__MODULE__, {:status, value})

  @impl true
  def init(_) do
    :ets.new(__MODULE__, [:named_table, :protected, read_concurrency: true])
    status("ready")
    {:ok, nil}
  end

  @impl true
  def handle_call({_op, _fun, _queued, _budget}, _from, port) when not is_nil(port),
    do:
      {:reply, {:error, "bd queue blocked: previous CLI exit unconfirmed; command not started"},
       port}

  def handle_call({op, fun, queued, budget}, {caller, _}, nil) do
    wait = now() - queued
    Logger.info("bd queue op=#{op} queue_ms=#{wait}")
    # Reserve response time inside the total budget, even with tiny test limits.
    deadline = queued + budget - min(2_000, div(budget, 10))

    cond do
      not Process.alive?(caller) ->
        {:reply, {:error, "bd caller disconnected; command not started"}, nil}

      wait > bounded(:bd_queue_timeout_ms, 8_000) or now() >= deadline ->
        {:reply, {:error, "bd queue wait exceeded; command not started"}, nil}

      true ->
        status("busy")
        Process.put(@deadline_key, deadline)

        {reply, returned_port} =
          try do
            {reply, port} = fun.()
            {reply, port}
          rescue
            _ -> internal_error(op)
          catch
            _, _ -> internal_error(op)
          end

        # An exception must never erase an active Port, even if no result exists.
        blocked = Process.get(@port_key) || returned_port

        if blocked do
          status("blocked")
          Logger.error("bd queue blocked: CLI exit unconfirmed; commands disabled")
        else
          status("ready")
        end

        {:reply, reply, blocked}
    end
  end

  defp internal_error(op) do
    Logger.error("bd internal error op=#{op}; checking active CLI before accepting work")
    {uncertain(op, "bd internal error"), Process.get(@port_key)}
  end

  defp uncertain(op, reason) when op in ~w(create claim update close) do
    {:indeterminate,
     %{
       "outcome" => "indeterminate",
       "operation" => op,
       "task_id" => nil,
       "retry_safe" => false,
       "message" =>
         reason <>
           "; the write may have committed. Do not retry automatically; wait and read back with show/list.",
       "readback" => %{"status" => "unavailable", "reason" => reason}
     }}
  end

  defp uncertain(_op, _reason),
    do: {:error, "bd internal error or deadline; no read result available"}

  @impl true
  def handle_info({port, {:exit_status, _}}, port) do
    confirm_exit(port)
    status("ready")
    Logger.error("bd queue unblocked: CLI exit confirmed; commands enabled")
    {:noreply, nil}
  end

  def handle_info(_msg, state), do: {:noreply, state}
end
