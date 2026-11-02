defmodule Aperture.Agents do
  @moduledoc """
  The standing agents, plus the members of any live team (`Aperture.Teams`).

  Each agent has a definition in `agents/<name>.json`: display name, role,
  prompt file, and a default harness (`claude` or `codex`) and model. The
  operator's own choice of harness and model per agent is saved in
  `data/agents.json` and wins over the defaults, so switching an agent never
  means editing code or definitions.

  Team members come from `data/teams.json`: harness and model are the ones the
  lead chose (the operator can still override them), `team` names their team
  and `cwd` is where they work (their worktree, or the team's project).
  """

  defmodule Agent do
    @moduledoc false
    defstruct [:name, :display, :role, :prompt_path, :harness, :model, :team, :cwd, mcp: %{}]

    @type t :: %__MODULE__{
            name: String.t(),
            display: String.t(),
            role: String.t(),
            prompt_path: Path.t(),
            harness: String.t(),
            model: String.t(),
            team: String.t() | nil,
            cwd: Path.t() | nil,
            mcp: %{
              String.t() => %{
                command: String.t(),
                args: [String.t()],
                env: %{String.t() => String.t()}
              }
            }
          }
  end

  @harnesses ~w(claude codex)

  @spec harnesses() :: [String.t()]
  def harnesses, do: @harnesses

  @doc "Known models per harness, from config `:models` (for the launcher's picker)."
  @spec models() :: %{String.t() => [String.t()]}
  def models do
    configured = Application.get_env(:aperture, :models, %{})
    Map.new(@harnesses, &{&1, Map.get(configured, &1, [])})
  end

  @spec list() :: [Agent.t()]
  def list do
    overrides = read_overrides()

    agents_dir()
    |> Path.join("*.json")
    |> Path.wildcard()
    |> Enum.sort()
    |> Enum.flat_map(&load(&1, overrides))
    |> Kernel.++(team_members(overrides))
  end

  defp team_members(overrides) do
    for team <- Aperture.Teams.list(), m <- team["members"] do
      o = Map.get(overrides, m["name"], %{})

      %Agent{
        name: m["name"],
        display: m["name"],
        role: "#{m["role"]} in team #{team["name"]}: " <> first_line(m["brief"]),
        prompt_path: nil,
        harness: pick(o["harness"], m["harness"], &(&1 in @harnesses), "claude"),
        model: pick(o["model"], m["model"], &valid_model?/1, nil),
        team: team["name"],
        cwd: m["cwd"]
      }
    end
  end

  defp first_line(text) do
    line = text |> String.split("\n", parts: 2) |> hd() |> String.trim()
    if String.length(line) > 140, do: String.slice(line, 0, 137) <> "...", else: line
  end

  @doc "Drop the operator's saved choice for `name` (a dissolved team member)."
  @spec forget(String.t()) :: :ok
  def forget(name) do
    overrides = read_overrides()

    if Map.has_key?(overrides, name) do
      path = overrides_path()
      tmp = path <> ".tmp"
      File.write!(tmp, JSON.encode!(Map.delete(overrides, name)))
      File.rename!(tmp, path)
    end

    :ok
  end

  @spec get(String.t()) :: Agent.t() | nil
  def get(name), do: Enum.find(list(), &(&1.name == name))

  @doc "Save the operator's harness and/or model for `name`."
  @spec set(String.t(), keyword()) :: {:ok, Agent.t()} | {:error, term()}
  def set(name, opts) do
    harness = Keyword.get(opts, :harness)
    model = Keyword.get(opts, :model)

    cond do
      get(name) == nil -> {:error, :unknown_agent}
      harness != nil and harness not in @harnesses -> {:error, :invalid_harness}
      model != nil and not valid_model?(model) -> {:error, :invalid_model}
      true -> save(name, harness, model)
    end
  end

  defp save(name, harness, model) do
    overrides = read_overrides()

    entry =
      overrides
      |> Map.get(name, %{})
      |> put_if("harness", harness)
      |> put_if("model", model)

    path = overrides_path()
    File.mkdir_p!(Path.dirname(path))
    tmp = path <> ".tmp"
    File.write!(tmp, JSON.encode!(Map.put(overrides, name, entry)))
    File.rename!(tmp, path)
    {:ok, get(name)}
  end

  defp put_if(map, _key, nil), do: map
  defp put_if(map, key, value), do: Map.put(map, key, value)

  @spec valid_model?(term()) :: boolean()
  def valid_model?(model),
    do: is_binary(model) and model =~ ~r/^[A-Za-z0-9][A-Za-z0-9._:\/\[\]-]{0,63}$/

  defp load(path, overrides) do
    with {:ok, body} <- File.read(path),
         {:ok, %{"name" => name} = d} <- JSON.decode(body),
         true <- name == Path.basename(path, ".json") and Aperture.Router.valid_name?(name) do
      o = Map.get(overrides, name, %{})

      [
        %Agent{
          name: name,
          display: d["display"] || name,
          role: d["role"] || "",
          prompt_path: Path.expand(d["prompt"] || "prompts/#{name}.md", root()),
          harness: pick(o["harness"], d["harness"], &(&1 in @harnesses), "claude"),
          model: pick(o["model"], d["model"], &valid_model?/1, nil),
          mcp: mcp_servers(d["mcp"])
        }
      ]
    else
      _ -> []
    end
  end

  defp mcp_servers(map) when is_map(map) do
    for {name, %{"command" => cmd} = spec} <- map,
        name =~ ~r/^[A-Za-z0-9_-]{1,64}$/ and name != "aperture",
        is_binary(cmd) and cmd != "",
        into: %{} do
      args = for a <- List.wrap(spec["args"]), is_binary(a), do: expand(a)

      env =
        for {k, v} <- Map.get(spec, "env") || %{},
            k =~ ~r/^[A-Za-z_][A-Za-z0-9_]*$/,
            is_binary(v),
            into: %{},
            do: {k, expand(v)}

      {name, %{command: expand(cmd), args: args, env: env}}
    end
  end

  defp mcp_servers(_), do: %{}

  defp expand("~/" <> rest), do: Path.join(System.user_home!(), rest)
  defp expand(value), do: value

  defp pick(override, default, valid?, fallback) do
    cond do
      override != nil and valid?.(override) -> override
      default != nil and valid?.(default) -> default
      true -> fallback
    end
  end

  defp read_overrides do
    with {:ok, body} <- File.read(overrides_path()),
         {:ok, map} when is_map(map) <- JSON.decode(body) do
      map
    else
      _ -> %{}
    end
  end

  defp root, do: Application.fetch_env!(:aperture, :root)
  defp agents_dir, do: Application.fetch_env!(:aperture, :agents_dir)
  defp overrides_path, do: Application.fetch_env!(:aperture, :agent_overrides)
end
