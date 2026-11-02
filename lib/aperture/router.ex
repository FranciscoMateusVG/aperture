defmodule Aperture.Router do
  @moduledoc """
  Routes a message: store it first, then push it if the recipient is connected.

  Returns `:pushed` when a live connection got it, `:queued` when the recipient
  is offline (it will be replayed when they connect).

  Team members cannot message the operator: they report to their lead, who
  decides what reaches Francisco.
  """

  alias Aperture.Store

  @spec send_message(String.t(), String.t(), String.t(), [Store.attachment()]) ::
          {:ok, integer(), :pushed | :queued} | {:error, String.t()}
  def send_message(from, to, body, attachments \\ [])

  def send_message(from, "operator", body, attachments) do
    case Aperture.Teams.member(from) do
      {team, _} -> {:error, "team members report to #{team["lead"]}, not the operator"}
      nil -> deliver(from, "operator", body, attachments)
    end
  end

  def send_message(from, to, body, attachments), do: deliver(from, to, body, attachments)

  defp deliver(from, to, body, attachments) do
    {:ok, msg} = Store.insert(from, to, body, attachments)
    Aperture.Chat.notify(msg)

    case Registry.lookup(Aperture.Connections, to) do
      [{pid, _}] ->
        send(pid, {:push, msg})
        {:ok, msg.id, :pushed}

      [] ->
        {:ok, msg.id, :queued}
    end
  end

  @spec connected_agents() :: [String.t()]
  def connected_agents do
    Registry.select(Aperture.Connections, [{{:"$1", :_, :_}, [], [:"$1"]}])
  end

  @doc """
  Register the calling process as the live connection for `agent`.

  A newer connection wins: the old one is sent `:replaced` and we wait for
  the Registry to drop it.
  """
  @spec register(String.t()) :: :ok
  def register(agent, attempts \\ 50) do
    case Registry.register(Aperture.Connections, agent, nil) do
      {:ok, _} ->
        :ok

      {:error, {:already_registered, self_pid}} when self_pid == self() ->
        :ok

      {:error, {:already_registered, old}} when attempts > 0 ->
        send(old, :replaced)
        Process.sleep(10)
        register(agent, attempts - 1)
    end
  end

  @spec unregister(String.t()) :: :ok
  def unregister(agent), do: Registry.unregister(Aperture.Connections, agent)

  @doc "Agent names are used in file paths and tmux targets, so keep them plain."
  @spec valid_name?(term()) :: boolean()
  def valid_name?(name) when is_binary(name), do: name =~ ~r/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/
  def valid_name?(_), do: false
end
