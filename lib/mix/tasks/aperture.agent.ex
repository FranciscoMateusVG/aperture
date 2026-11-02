defmodule Mix.Tasks.Aperture.Agent do
  @shortdoc "Show agents, or change one agent's harness/model"
  @moduledoc """
      mix aperture.agent                                   # list agents
      mix aperture.agent wheatley --harness codex --model gpt-6-astra
      mix aperture.agent glados --model sonnet

  If the hub is running, the agent is restarted with the new settings right
  away. Otherwise the choice is saved and used at the next `mix aperture.up`.
  """
  use Mix.Task

  @impl true
  def run(args) do
    Mix.Task.run("app.config")
    {opts, rest, _} = OptionParser.parse(args, strict: [harness: :string, model: :string])

    case rest do
      [] -> list()
      [name] when opts == [] -> list(name)
      [name] -> change(name, opts)
      _ -> Mix.raise("usage: mix aperture.agent [NAME [--harness claude|codex] [--model MODEL]]")
    end
  end

  defp list(only \\ nil) do
    for a <- Aperture.Agents.list(), only in [nil, a.name] do
      Mix.shell().info(
        "#{String.pad_trailing(a.name, 10)} #{String.pad_trailing(a.harness, 7)} #{a.model}"
      )
    end
  end

  defp change(name, opts) do
    body = JSON.encode!(Map.new(opts))

    case post("/agents/#{name}/config", body) do
      {:ok, 202, _} ->
        Mix.shell().info("#{name}: saved, restarting on the running hub")

      {:ok, status, resp} ->
        Mix.raise("hub refused (#{status}): #{resp}")

      {:error, _} ->
        case Aperture.Agents.set(name, opts) do
          {:ok, a} ->
            Mix.shell().info(
              "#{name}: saved (#{a.harness} #{a.model}); hub not running, used at next start"
            )

          {:error, reason} ->
            Mix.raise("#{name}: #{reason}")
        end
    end
  end

  defp post(path, body) do
    Application.ensure_all_started(:mint)
    port = Application.fetch_env!(:aperture, :port)

    with {:ok, conn} <- Mint.HTTP.connect(:http, "127.0.0.1", port, mode: :passive),
         {:ok, conn, ref} <-
           Mint.HTTP.request(conn, "POST", path, [{"content-type", "application/json"}], body),
         {:ok, status, resp} <- read(conn, ref, nil, "") do
      {:ok, status, resp}
    else
      _ -> {:error, :hub_unreachable}
    end
  end

  defp read(conn, ref, status, acc) do
    case Mint.HTTP.recv(conn, 0, 5_000) do
      {:ok, conn, responses} ->
        {status, acc, done} =
          Enum.reduce(responses, {status, acc, false}, fn
            {:status, ^ref, s}, {_, a, d} -> {s, a, d}
            {:data, ^ref, d}, {s, a, done} -> {s, a <> d, done}
            {:done, ^ref}, {s, a, _} -> {s, a, true}
            _, x -> x
          end)

        if done, do: {:ok, status, acc}, else: read(conn, ref, status, acc)

      _ ->
        {:error, :recv}
    end
  end
end
