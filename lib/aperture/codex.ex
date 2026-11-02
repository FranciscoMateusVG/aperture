defmodule Aperture.Codex do
  @moduledoc """
  Codex agents. One supervised `Aperture.Codex.Bridge` per agent connects to
  that agent's `codex app-server --listen unix://<socket>` and delivers its
  messages as turns.
  """

  alias Aperture.Codex.Bridge

  @doc "Start (or keep) the bridge for `agent` on `socket`. Idempotent."
  @spec start_bridge(String.t(), Path.t()) :: {:ok, pid()} | {:error, term()}
  def start_bridge(agent, socket) do
    cond do
      not Aperture.Router.valid_name?(agent) ->
        {:error, :invalid_agent}

      not is_binary(socket) or socket == "" ->
        {:error, :invalid_socket}

      true ->
        case DynamicSupervisor.start_child(
               Aperture.Codex.Supervisor,
               {Bridge, agent: agent, socket: socket}
             ) do
          {:ok, pid} -> {:ok, pid}
          {:error, {:already_started, pid}} -> {:ok, pid}
          other -> other
        end
    end
  end

  @spec stop_bridge(String.t()) :: :ok | {:error, :not_found}
  def stop_bridge(agent) do
    case Registry.lookup(Aperture.Codex.Bridges, agent) do
      [{pid, _}] -> DynamicSupervisor.terminate_child(Aperture.Codex.Supervisor, pid)
      [] -> {:error, :not_found}
    end
  end

  @doc """
  The Codex thread this agent owns. Written when its bridge starts a thread,
  kept across restarts, and used by the operator's pane for
  `codex resume <id> --remote`.
  """
  @spec thread_id_path(String.t()) :: Path.t()
  def thread_id_path(agent),
    do: Path.join(Application.fetch_env!(:aperture, :run_dir), "#{agent}.thread-id")
end
