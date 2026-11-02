defmodule Aperture.Teams do
  @moduledoc """
  Teams: agents a lead (GLaDOS) creates for one piece of work and dissolves
  when it is done.

  A team has a name, a project directory and 1..`:team_max_members` members.
  The lead chooses every member's role, brief, harness and model. A member
  that writes code (`worktree: true`) gets its own git worktree of the project
  on a new branch `team/<team>/<role>` (suffixed `-2`, `-3`… if taken); the
  others work in the project directory itself.

  Members are ordinary agents named `<team>-<role>`: they show up in
  `Aperture.Agents.list/0`, launch through `Aperture.Launcher` (in parallel),
  appear on the launcher page and in tmux. They report to their lead and
  cannot message the operator (`Aperture.Router`).

  Dissolving stops the members, removes worktrees that are clean, keeps any
  worktree with uncommitted or untracked work (and says where), and never
  deletes a branch.

  Teams live in `data/teams.json` (atomic writes). Changes go through this
  GenServer, one at a time; reads go straight to the file.
  """
  use GenServer
  require Logger

  @name_re ~r/^[a-z][a-z0-9-]{0,19}$/
  @max_brief 8_000

  def start_link(_opts), do: GenServer.start_link(__MODULE__, nil, name: __MODULE__)

  @doc "Create a team and launch its members. `by` must be a lead."
  @spec create(String.t(), map()) :: {:ok, map()} | {:error, atom(), String.t()}
  def create(by, spec), do: GenServer.call(__MODULE__, {:create, by, spec}, :infinity)

  @doc "Dissolve a team. `by` is its lead or `operator`."
  @spec dissolve(String.t(), String.t()) :: {:ok, map()} | {:error, atom(), String.t()}
  def dissolve(by, team), do: GenServer.call(__MODULE__, {:dissolve, by, team}, :infinity)

  @spec list() :: [map()]
  def list, do: read() |> Map.values() |> Enum.sort_by(& &1["created_at"])

  @spec get(String.t()) :: map() | nil
  def get(name), do: Map.get(read(), name)

  @doc "The team and member entry for agent `name`, or nil for a standing agent."
  @spec member(String.t()) :: {map(), map()} | nil
  def member(name) do
    Enum.find_value(read(), fn {_, team} ->
      case Enum.find(team["members"], &(&1["name"] == name)) do
        nil -> nil
        m -> {team, m}
      end
    end)
  end

  @spec lead?(String.t()) :: boolean()
  def lead?(agent), do: agent in Application.get_env(:aperture, :team_leads, ["glados"])

  # -- GenServer ----------------------------------------------------------------

  @impl true
  def init(nil), do: {:ok, nil}

  @impl true
  def handle_call({:create, by, spec}, _from, state) do
    reply =
      with :ok <- check_lead(by),
           {:ok, team} <- validate(by, spec),
           {:ok, team} <- make_worktrees(team) do
        write(Map.put(read(), team["name"], team))
        started = Aperture.Launcher.start_many(Enum.map(team["members"], & &1["name"]))
        {:ok, summary(team, started)}
      end

    {:reply, reply, state}
  end

  def handle_call({:dissolve, by, name}, _from, state) do
    reply =
      case get(name) do
        nil ->
          {:error, :not_found, "no team #{name}"}

        team when by == "operator" or by == :operator ->
          {:ok, do_dissolve(team)}

        team ->
          if by == team["lead"],
            do: {:ok, do_dissolve(team)},
            else:
              {:error, :forbidden, "only #{team["lead"]} or the operator can dissolve #{name}"}
      end

    {:reply, reply, state}
  end

  # -- create -------------------------------------------------------------------

  defp check_lead(by) do
    if is_binary(by) and lead?(by),
      do: :ok,
      else:
        {:error, :forbidden, "only a team lead (#{Enum.join(leads(), ", ")}) can create teams"}
  end

  defp validate(by, spec) when is_map(spec) do
    name = spec["name"]
    members = spec["members"]
    max = Application.get_env(:aperture, :team_max_members, 6)

    with :ok <-
           check(
             is_binary(name) and name =~ @name_re,
             "team name: lowercase letters, digits and -, up to 20, starting with a letter"
           ),
         :ok <- check(get(name) == nil, "team #{name} already exists"),
         {:ok, project} <- project(spec["project"]),
         :ok <- check(is_list(members) and members != [], "members: at least one"),
         :ok <- check(length(members) <= max, "members: at most #{max}"),
         {:ok, members} <- members(name, members, project) do
      {:ok,
       %{
         "name" => name,
         "lead" => by,
         "project" => project,
         "base" =>
           if(is_binary(spec["base"]) and spec["base"] != "", do: spec["base"], else: "HEAD"),
         "created_at" => DateTime.utc_now() |> DateTime.to_iso8601(),
         "members" => members
       }}
    end
  end

  defp validate(_by, _spec), do: invalid("expected a team object")

  defp project(path) when is_binary(path) and path != "" do
    full = expand(path)

    cond do
      Path.type(full) != :absolute -> invalid("project must be an absolute path")
      not File.dir?(Path.expand(full)) -> invalid("project #{full} is not a directory")
      true -> {:ok, Path.expand(full)}
    end
  end

  defp project(_), do: invalid("project: the directory the team works on")

  defp members(team, members, project) do
    roles = Enum.map(members, &(is_map(&1) && &1["role"]))

    with :ok <- check(Enum.uniq(roles) == roles, "member roles must be unique"),
         {:ok, list} <- collect(members, &member(team, &1, project)) do
      if Enum.any?(list, & &1["worktree"]) and not git_repo?(project),
        do: invalid("worktree members need a git repository; #{project} is not one"),
        else: {:ok, list}
    end
  end

  defp member(team, m, _project) when is_map(m) do
    role = m["role"]
    name = "#{team}-#{role}"
    models_hint = "e.g. " <> Enum.join(Aperture.Agents.models()[m["harness"]] || [], ", ")

    with :ok <-
           check(
             is_binary(role) and role =~ @name_re,
             "role: lowercase slug like coder or reviewer"
           ),
         :ok <-
           check(
             Aperture.Router.valid_name?(name) and String.length(name) <= 40,
             "#{name}: name too long"
           ),
         :ok <-
           check(
             Aperture.Agents.get(name) == nil and name != "operator",
             "#{name} already exists"
           ),
         :ok <-
           check(
             m["harness"] in Aperture.Agents.harnesses(),
             "#{role}: harness must be claude or codex"
           ),
         :ok <-
           check(
             Aperture.Agents.valid_model?(m["model"]),
             "#{role}: choose a model (#{models_hint})"
           ),
         :ok <-
           check(
             is_binary(m["brief"]) and String.trim(m["brief"]) != "",
             "#{role}: brief is required"
           ),
         :ok <-
           check(
             String.length(m["brief"]) <= @max_brief,
             "#{role}: brief over #{@max_brief} characters"
           ) do
      {:ok,
       %{
         "name" => name,
         "role" => role,
         "brief" => String.trim(m["brief"]),
         "harness" => m["harness"],
         "model" => m["model"],
         "worktree" => m["worktree"] == true
       }}
    end
  end

  defp member(_team, _m, _project), do: invalid("each member is an object")

  # Worktrees first, all or nothing: if one fails, the ones made so far are
  # removed again (they are brand new, so nothing is lost).
  defp make_worktrees(team) do
    result =
      Enum.reduce_while(team["members"], {:ok, []}, fn m, {:ok, done} ->
        if m["worktree"] do
          case add_worktree(team, m) do
            {:ok, m} -> {:cont, {:ok, [m | done]}}
            {:error, msg} -> {:halt, {:error, msg, done}}
          end
        else
          {:cont, {:ok, [Map.put(m, "cwd", team["project"]) | done]}}
        end
      end)

    case result do
      {:ok, members} ->
        {:ok, %{team | "members" => Enum.reverse(members)}}

      {:error, msg, done} ->
        for m <- done, m["branch"], do: undo_worktree(team["project"], m)
        invalid(msg)
    end
  end

  defp add_worktree(team, m) do
    dir = Path.join(worktrees_dir(), m["name"])
    branch = free_branch(team["project"], "team/#{team["name"]}/#{m["role"]}")
    File.mkdir_p!(worktrees_dir())

    cond do
      File.exists?(dir) ->
        {:error, "#{dir} already exists (left over from an earlier team?)"}

      true ->
        case git(team["project"], ["worktree", "add", "-b", branch, dir, team["base"]]) do
          {:ok, _} -> {:ok, Map.merge(m, %{"cwd" => dir, "branch" => branch})}
          {:error, out} -> {:error, "#{m["role"]}: git worktree add failed: #{String.trim(out)}"}
        end
    end
  end

  defp undo_worktree(project, m) do
    _ = git(project, ["worktree", "remove", "--force", m["cwd"]])
    _ = git(project, ["branch", "-D", m["branch"]])
  end

  defp free_branch(project, base) do
    Stream.iterate(1, &(&1 + 1))
    |> Stream.map(fn
      1 -> base
      n -> "#{base}-#{n}"
    end)
    |> Enum.find(&(not branch?(project, &1)))
  end

  defp branch?(project, branch),
    do:
      match?(
        {:ok, _},
        git(project, ["rev-parse", "--verify", "--quiet", "refs/heads/" <> branch])
      )

  defp git_repo?(project), do: match?({:ok, _}, git(project, ["rev-parse", "--git-dir"]))

  defp summary(team, started) do
    %{
      "team" => team["name"],
      "project" => team["project"],
      "members" =>
        for m <- team["members"] do
          %{
            "name" => m["name"],
            "role" => m["role"],
            "harness" => m["harness"],
            "model" => m["model"],
            "cwd" => m["cwd"],
            "branch" => m["branch"],
            "status" => status(Map.get(started, m["name"]))
          }
          |> Map.reject(fn {_, v} -> is_nil(v) end)
        end
    }
  end

  defp status({:ok, :started}), do: "started"
  defp status({:ok, :already_running}), do: "started"
  defp status({:error, reason}), do: "failed: #{inspect(reason)}"
  defp status(other), do: "failed: #{inspect(other)}"

  # -- dissolve -----------------------------------------------------------------

  defp do_dissolve(team) do
    names = Enum.map(team["members"], & &1["name"])
    Aperture.Launcher.stop_many(names)

    members =
      for m <- team["members"] do
        Aperture.Agents.forget(m["name"])
        File.rm(Aperture.Codex.thread_id_path(m["name"]))
        Map.merge(%{"name" => m["name"]}, cleanup_worktree(team, m))
      end

    write(Map.delete(read(), team["name"]))
    %{"team" => team["name"], "dissolved" => true, "members" => members}
  end

  # `git worktree remove` without --force refuses when there are uncommitted
  # or untracked files: exactly the work we must not throw away.
  defp cleanup_worktree(team, %{"branch" => branch, "cwd" => dir}) do
    cond do
      not File.exists?(dir) ->
        _ = git(team["project"], ["worktree", "prune"])
        %{"branch" => branch, "worktree" => "gone"}

      match?({:ok, _}, git(team["project"], ["worktree", "remove", dir])) ->
        %{"branch" => branch, "worktree" => "removed"}

      true ->
        %{"branch" => branch, "worktree" => "kept (uncommitted changes): #{dir}"}
    end
  end

  defp cleanup_worktree(_team, _m), do: %{}

  # -- storage & helpers --------------------------------------------------------

  defp read do
    with {:ok, body} <- File.read(path()),
         {:ok, map} when is_map(map) <- JSON.decode(body) do
      map
    else
      _ -> %{}
    end
  end

  defp write(teams) do
    File.mkdir_p!(Path.dirname(path()))
    tmp = path() <> ".tmp"
    File.write!(tmp, JSON.encode!(teams))
    File.rename!(tmp, path())
  end

  defp git(dir, args) do
    case System.cmd("git", ["-C", dir | args], stderr_to_stdout: true) do
      {out, 0} -> {:ok, String.trim(out)}
      {out, _} -> {:error, out}
    end
  rescue
    e -> {:error, Exception.message(e)}
  end

  defp collect(items, fun) do
    Enum.reduce_while(items, {:ok, []}, fn item, {:ok, acc} ->
      case fun.(item) do
        {:ok, x} -> {:cont, {:ok, [x | acc]}}
        error -> {:halt, error}
      end
    end)
    |> case do
      {:ok, list} -> {:ok, Enum.reverse(list)}
      error -> error
    end
  end

  defp check(true, _msg), do: :ok
  defp check(_, msg), do: invalid(msg)

  defp invalid(msg), do: {:error, :invalid, msg}

  defp expand("~/" <> rest), do: Path.join(System.user_home!(), rest)
  defp expand(path), do: path

  defp leads, do: Application.get_env(:aperture, :team_leads, ["glados"])
  defp path, do: Application.fetch_env!(:aperture, :teams_path)
  defp worktrees_dir, do: Application.fetch_env!(:aperture, :worktrees_dir)
end
