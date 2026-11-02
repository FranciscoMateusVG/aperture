defmodule Mix.Tasks.Aperture.Up do
  @shortdoc "Start the hub and every agent (each in its tmux window)"
  @moduledoc """
  Starts the hub, then launches every agent in `agents/` with its chosen
  harness and model, each in its own window of the `aperture-v4` tmux session.
  Keeps running until you stop it (Ctrl+C twice); agents keep their windows.

      mix aperture.up
      tmux attach -t aperture-v4
  """
  use Mix.Task

  @impl true
  def run(_args) do
    Mix.Task.run("app.start")
    session = Application.fetch_env!(:aperture, :tmux_session)

    for {name, result} <- Aperture.Launcher.start_all() do
      Mix.shell().info("#{String.pad_trailing(name, 10)} #{format(result)}")
    end

    Mix.shell().info("\nAgents are in tmux session #{session}:  tmux attach -t #{session}")
    Process.sleep(:infinity)
  end

  defp format({:ok, :started}), do: "started"
  defp format({:ok, :already_running}), do: "already running"
  defp format({:error, reason}), do: "FAILED: #{inspect(reason)}"
end
