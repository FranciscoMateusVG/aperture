defmodule Aperture.Beads do
  @moduledoc """
  BEADS task tracking for agents, through the `bd` CLI.

  Only the hub runs `bd`: agents call it via `POST /beads/:op` (the aperture
  MCP tools do that), so the Dolt credentials stay in the hub's environment
  and never reach an agent's config. `BD_ACTOR` is the calling agent.
  Uses v3's `~/.aperture/.beads` by default, so existing tasks carry over.
  """

  @ops ~w(create list show claim update close)
  @statuses ~w(open in_progress blocked closed)
  @summary ~w(id title status priority assignee labels)
  @detail_cap 4_000

  def ops, do: @ops

  @spec run(String.t(), String.t(), map()) ::
          {:ok, term()} | {:error, String.t()} | {:indeterminate, map()}
  def run(op, actor, params) when op in @ops do
    if Aperture.Router.valid_name?(actor) do
      with {:ok, args} <- args(op, actor, params) do
        Aperture.Beads.Queue.run(op, fn -> execute(op, args, actor, params) end)
      end
    else
      {:error, "invalid agent"}
    end
  end

  def run(_op, _actor, _params), do: {:error, "unknown operation"}

  # -- argv per operation (never a shell; flags only from fixed names) --------

  # Title as --title=value, like all free text: taken literally even if it starts with "-".
  defp args("create", _actor, p) do
    with {:ok, title} <- text(p["title"], "title"),
         {:ok, prio} <- priority(p["priority"]) do
      {:ok,
       ["create", "--title=" <> title, "-p", to_string(prio), "--json"] ++
         eq("--description", p["description"]) ++
         opt("-l", labels(p["labels"])) ++
         opt("--assignee", name(p["assignee"])) ++
         eq("--acceptance", p["acceptance"])}
    end
  end

  defp args("list", _actor, p) do
    mode = if p["ready"] == true, do: "ready", else: "list"

    status =
      cond do
        mode == "ready" -> []
        p["status"] in @statuses -> ["--status", p["status"]]
        true -> []
      end

    {:ok,
     [mode, "--json"] ++
       status ++
       opt("--assignee", name(p["assignee"])) ++
       opt("--label", label(p["label"])) ++
       ["-n", to_string(limit(p["limit"]))]}
  end

  defp args("show", _actor, p),
    do: with({:ok, id} <- id(p["id"]), do: {:ok, ["show", id, "--json"]})

  defp args("claim", actor, p) do
    with {:ok, id} <- id(p["id"]),
         do: {:ok, ["update", id, "--status", "in_progress", "--assignee", actor, "--json"]}
  end

  defp args("update", _actor, p) do
    with {:ok, id} <- id(p["id"]) do
      status =
        if p["status"] in (@statuses -- ["closed"]), do: ["--status", p["status"]], else: []

      flags =
        status ++
          opt("--assignee", name(p["assignee"])) ++
          eq("--append-notes", p["notes"]) ++
          Enum.flat_map(List.wrap(p["add_labels"]), &opt("--add-label", label(&1)))

      if flags == [],
        do: {:error, "nothing to update"},
        else: {:ok, ["update", id] ++ flags ++ ["--json"]}
    end
  end

  defp args("close", _actor, p) do
    with {:ok, id} <- id(p["id"]),
         {:ok, reason} <- text(p["reason"], "reason"),
         do: {:ok, ["close", id, "--reason=" <> reason, "--json"]}
  end

  # -- validation ---------------------------------------------------------------

  defp text(v, field) when is_binary(v) do
    case String.trim(v) do
      "" -> {:error, "#{field} is required"}
      t -> {:ok, t}
    end
  end

  defp text(_, field), do: {:error, "#{field} is required"}

  defp id(v) when is_binary(v) do
    if v =~ ~r/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/, do: {:ok, v}, else: {:error, "invalid task id"}
  end

  defp id(_), do: {:error, "task id is required"}

  defp priority(nil), do: {:ok, 2}
  defp priority(p) when p in 0..4, do: {:ok, p}
  defp priority(_), do: {:error, "priority must be 0-4"}

  defp limit(n) when is_integer(n) and n in 1..200, do: n
  defp limit(_), do: 50

  defp name(v), do: if(Aperture.Router.valid_name?(v), do: v)
  defp label(v) when is_binary(v), do: if(v =~ ~r/^[A-Za-z0-9][A-Za-z0-9:_.-]{0,63}$/, do: v)
  defp label(_), do: nil

  defp labels(list) when is_list(list) do
    case list |> Enum.map(&label/1) |> Enum.reject(&is_nil/1) do
      [] -> nil
      ok -> Enum.join(ok, ",")
    end
  end

  defp labels(_), do: nil

  # Free text as --flag=value, so text starting with "-" is never read as a flag.
  defp eq(flag, v) when is_binary(v) and v != "", do: [flag <> "=" <> v]
  defp eq(_flag, _), do: []

  defp opt(_flag, nil), do: []
  defp opt(_flag, ""), do: []
  defp opt(flag, v) when is_binary(v), do: [flag, v]
  defp opt(_flag, _), do: []

  # -- running bd ---------------------------------------------------------------

  defp execute(op, args, actor, params) do
    result = Aperture.Beads.Command.run(args, actor, op)

    case result do
      %{kind: :exit, code: 0, out: out} ->
        reply = with {:ok, json} <- decode(out), do: {:ok, shape(op, json)}
        {reply, nil}

      %{kind: :exit, code: code, out: out, err: err} ->
        {{:error, "bd exited #{code}: #{String.slice(String.trim(err <> " " <> out), 0, 500)}"},
         nil}

      %{kind: :timeout} when op in ~w(create claim update close) ->
        recover(op, actor, params, result)

      %{kind: :timeout, blocked: blocked} ->
        {{:error, "bd read timed out; no result available"}, blocked}

      %{kind: :launch_error} ->
        {{:error, "bd could not be started"}, nil}
    end
  end

  defp recover(op, actor, params, result) do
    # A create without a receipt cannot be uniquely identified by title.
    # Never reissue a write, even when readback fails or shows the old state.
    candidate = if op == "create", do: receipt_id(result.out), else: params["id"]

    {readback, blocked} =
      if is_binary(candidate) and is_nil(result.blocked) do
        started = System.monotonic_time(:millisecond)
        read = Aperture.Beads.Command.run(["show", candidate, "--json"], actor, "readback")

        observed =
          with %{kind: :exit, code: 0} <- read,
               {:ok, json} <- JSON.decode(read.out),
               task when is_map(task) <- shape("show", json),
               true <- task["id"] == candidate do
            %{"status" => "observed", "task" => task}
          else
            _ -> %{"status" => "unavailable", "reason" => "readback failed"}
          end

        require Logger

        Logger.info(
          "bd recovery op=#{op} elapsed_ms=#{System.monotonic_time(:millisecond) - started} outcome=#{observed["status"]}"
        )

        {observed, read.blocked}
      else
        reason = if candidate, do: "CLI exit unconfirmed", else: "no task id in stdout receipt"
        {%{"status" => "unavailable", "reason" => reason}, result.blocked}
      end

    {{:indeterminate,
      %{
        "outcome" => "indeterminate",
        "operation" => op,
        "task_id" => candidate,
        "retry_safe" => false,
        "message" =>
          "bd timed out; the write may have committed. Readback is an observation, not proof of completion. Do not retry the mutation automatically.",
        "readback" => readback
      }}, blocked}
  end

  defp receipt_id(out) do
    with {:ok, json} <- JSON.decode(out),
         candidate <- extract_id(json),
         {:ok, valid} <- id(candidate) do
      valid
    else
      _ -> nil
    end
  end

  defp extract_id([one]), do: extract_id(one)
  defp extract_id(%{"id" => id}), do: id
  defp extract_id(%{"issue" => issue}), do: extract_id(issue)
  defp extract_id(%{"task" => task}), do: extract_id(task)
  defp extract_id(_), do: nil

  defp decode(out) do
    case JSON.decode(out) do
      {:ok, json} -> {:ok, json}
      {:error, _} -> {:error, "bd returned non-JSON output: #{String.slice(out, 0, 300)}"}
    end
  end

  # -- keep results small: agents pay for every token ---------------------------

  defp shape("list", rows) when is_list(rows), do: Enum.map(rows, &Map.take(&1, @summary))
  defp shape("show", [task]), do: shape("show", task)

  defp shape("show", task) when is_map(task) do
    Map.update(task, "description", nil, fn
      d when is_binary(d) and byte_size(d) > @detail_cap ->
        String.slice(d, 0, @detail_cap) <> "\n[truncated]"

      d ->
        d
    end)
  end

  defp shape(_op, [single]) when is_map(single), do: Map.take(single, @summary)
  defp shape(_op, %{} = one), do: Map.take(one, @summary ++ ["issue", "task"])
  defp shape(_op, other), do: other
end
