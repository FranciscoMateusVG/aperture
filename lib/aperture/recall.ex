defmodule Aperture.Recall do
  @moduledoc """
  Read-only access to v3's recall memory: the BEADS memory bank as v3 indexed
  it, in `~/.aperture/run/memory-index.json` (credentials already redacted by
  v3, every entry flagged secret or not, superseded entries marked).

  Secret entries are never returned. Superseded entries are hidden. Nothing is
  ever written: v3's file stays the source. Reloaded when the file changes.
  """

  @full_max_bytes 8 * 1024
  @k_max 10
  @standing_boost 1.5

  @spec search(String.t(), pos_integer()) :: {:ok, [map()]} | {:error, String.t()}
  def search(query, k \\ 5)

  def search(query, k) when is_binary(query) and query != "" do
    with {:ok, entries} <- entries() do
      terms = tokens(query)
      exact = String.downcase(String.trim(query))

      results =
        entries
        |> Enum.map(fn e -> {score(e, terms, exact), e} end)
        |> Enum.filter(fn {s, _} -> s > 0 end)
        |> Enum.sort_by(fn {s, e} -> {-s, e.key} end)
        |> Enum.take(min(max(k, 1), @k_max))
        |> Enum.map(fn {s, e} ->
          %{key: e.key, gist: e.gist, tags: e.tags, project: e.project, score: Float.round(s, 3)}
        end)

      {:ok, results}
    end
  end

  def search(_, _), do: {:error, "query is required"}

  @spec full(String.t()) :: {:ok, map()} | {:error, String.t()}
  def full(key) when is_binary(key) do
    with {:ok, entries} <- entries() do
      case Enum.find(entries, &(&1.key == key)) do
        nil ->
          {:error, "no such memory"}

        e ->
          truncated = byte_size(e.body) > @full_max_bytes

          body =
            if truncated,
              do: binary_slice(e.body, 0, @full_max_bytes) <> "\n[truncated]",
              else: e.body

          {:ok,
           %{
             key: e.key,
             body: body,
             tags: e.tags,
             project: e.project,
             updated: e.updated,
             truncated: truncated
           }}
      end
    end
  end

  def full(_), do: {:error, "key is required"}

  # -- loading (only live, non-secret entries ever leave this function) ---------

  defp entries do
    path = Application.fetch_env!(:aperture, :memory_index)

    with {:ok, %File.Stat{mtime: mtime}} <- File.stat(path) do
      case :persistent_term.get({__MODULE__, path}, nil) do
        {^mtime, entries} -> {:ok, entries}
        _ -> load(path, mtime)
      end
    else
      _ -> {:error, "v3 memory index not found at #{path}"}
    end
  end

  defp load(path, mtime) do
    with {:ok, raw} <- File.read(path),
         {:ok, %{"entries" => list} = index} when is_list(list) <- JSON.decode(raw) do
      gists = Map.new(index["lines"] || [], &{&1["key"], &1["gist"]})

      entries =
        for %{"key" => key, "body" => body} = e <- list,
            is_binary(key) and is_binary(body),
            e["secret"] != true,
            is_nil(e["supersededBy"]) do
          meta = e["meta"] || %{}

          %{
            key: key,
            body: body,
            gist: gists[key] || String.slice(body, 0, 80),
            tags: meta["tags"] || [],
            project: meta["project"],
            updated: meta["updated"],
            standing: meta["standing"] == true,
            terms:
              Enum.frequencies(
                tokens(key <> " " <> body <> " " <> Enum.join(meta["tags"] || [], " "))
              )
          }
        end

      :persistent_term.put({__MODULE__, path}, {mtime, entries})
      {:ok, entries}
    else
      _ -> {:error, "v3 memory index is unreadable"}
    end
  end

  # -- scoring: term overlap, key hits weigh more, exact key wins ---------------

  defp score(e, terms, exact) do
    base =
      terms
      |> Enum.uniq()
      |> Enum.reduce(0.0, fn t, acc ->
        tf = Map.get(e.terms, t, 0)
        key_hit = if String.contains?(e.key, t), do: 2.0, else: 0.0
        acc + if(tf > 0, do: 1.0 + :math.log(tf), else: 0.0) + key_hit
      end)

    base = if e.standing, do: base * @standing_boost, else: base
    if e.key == exact, do: base + 100.0, else: base
  end

  defp tokens(text) do
    text
    |> String.downcase()
    |> then(&Regex.scan(~r/[a-z0-9][a-z0-9_-]*/u, &1))
    |> List.flatten()
    |> Enum.flat_map(fn w -> [w | String.split(w, ["-", "_"], trim: true)] end)
    |> Enum.filter(&(String.length(&1) >= 2))
  end
end
