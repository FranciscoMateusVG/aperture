defmodule Aperture.Web.TeamTasks do
  @moduledoc """
  Each team member's current BEADS task (in_progress, by assignee), for the
  team panels on the launcher page.

  `bd` is slow (Dolt), so this never runs on the page's clock: a refresh
  every `:team_tasks_every_ms` (30s; nil in tests: only `refresh/0`) in a
  separate task, only while there are teams, and the page reads the last
  result (`current/0`).
  """
  use GenServer
  require Logger

  def start_link(_opts), do: GenServer.start_link(__MODULE__, nil, name: __MODULE__)

  @doc "`%{assignee => %{id, title}}` from the last refresh (empty until the first one)."
  @spec current() :: %{String.t() => %{id: String.t(), title: String.t()}}
  def current do
    GenServer.call(__MODULE__, :current, 1_000)
  catch
    :exit, _ -> %{}
  end

  @doc "Refresh now and wait for it (tests, and when a team appears)."
  @spec refresh() :: :ok
  def refresh, do: GenServer.call(__MODULE__, :refresh, 60_000)

  @doc false
  def index(tasks) do
    for %{"assignee" => who, "id" => id, "status" => "in_progress"} = t <- tasks,
        is_binary(who) and who != "",
        into: %{} do
      {who, %{id: id, title: t["title"] || ""}}
    end
  end

  @impl true
  def init(nil) do
    if every_ms(), do: send(self(), :tick)
    {:ok, %{tasks: %{}, task: nil}}
  end

  @impl true
  def handle_call(:current, _from, state), do: {:reply, state.tasks, state}

  def handle_call(:refresh, _from, state), do: {:reply, :ok, %{state | tasks: fetch(state.tasks)}}

  @impl true
  def handle_info(:tick, %{task: nil} = state) do
    Process.send_after(self(), :tick, every_ms())

    if Aperture.Teams.list() == [] do
      {:noreply, state}
    else
      %Task{ref: ref} = Task.async(fn -> fetch(state.tasks) end)
      {:noreply, %{state | task: ref}}
    end
  end

  def handle_info(:tick, state) do
    Process.send_after(self(), :tick, every_ms())
    {:noreply, state}
  end

  def handle_info({ref, tasks}, %{task: ref} = state) do
    Process.demonitor(ref, [:flush])
    {:noreply, %{state | tasks: tasks, task: nil}}
  end

  def handle_info({:DOWN, ref, _, _, _}, %{task: ref} = state),
    do: {:noreply, %{state | task: nil}}

  def handle_info(_other, state), do: {:noreply, state}

  defp every_ms, do: Application.get_env(:aperture, :team_tasks_every_ms, 30_000)

  # keeps the last good result when bd fails
  defp fetch(previous) do
    case Aperture.Beads.run("list", "operator", %{"status" => "in_progress", "limit" => 200}) do
      {:ok, tasks} when is_list(tasks) ->
        index(tasks)

      _other ->
        Logger.warning("team tasks: bd list failed; retaining previous result")
        previous
    end
  end
end
