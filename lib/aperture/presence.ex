defmodule Aperture.Presence do
  @moduledoc """
  Busy/idle state per agent.

  Claude agents report it through Claude Code hooks (`bin/aperture-state.mjs`,
  posted to `/presence/:agent`). Codex agents report it from their bridge,
  which follows Codex's turn notifications. Whether an agent is online at all
  comes from the connection registry, not from here.
  """
  use Agent

  def start_link(_opts), do: Agent.start_link(fn -> %{} end, name: __MODULE__)

  @spec set(String.t(), :busy | :idle) :: :ok
  def set(agent, state) when state in [:busy, :idle],
    do: Agent.update(__MODULE__, &Map.put(&1, agent, state))

  @spec clear(String.t()) :: :ok
  def clear(agent), do: Agent.update(__MODULE__, &Map.delete(&1, agent))

  @doc "Connected presence, or the last content-free startup outcome."
  @spec state(String.t()) :: :offline | :busy | :idle | :starting | :startup_failed
  def state(agent) do
    if agent in Aperture.Router.connected_agents() do
      Agent.get(__MODULE__, &Map.get(&1, agent, :idle))
    else
      case Aperture.Launcher.lifecycle(agent).state do
        state when state in [:starting, :startup_failed] -> state
        _ -> :offline
      end
    end
  end
end
