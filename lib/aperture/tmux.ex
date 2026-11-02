defmodule Aperture.Tmux do
  @moduledoc "Thin wrapper over the tmux CLI. tmux is the operator's console only."

  @spec ensure_session(String.t()) :: :ok
  def ensure_session(session) do
    case run(["has-session", "-t", session]) do
      {_, 0} ->
        :ok

      _ ->
        {_, 0} = run(["new-session", "-d", "-s", session, "-n", "console"])
        :ok
    end
  end

  @doc "Open a background window running `command` (a shell command string)."
  @spec new_window(String.t(), String.t(), String.t()) :: :ok
  def new_window(session, name, command) do
    {_, 0} = run(["new-window", "-d", "-t", session <> ":", "-n", name, command])
    :ok
  end

  @spec kill_window(String.t(), String.t()) :: :ok
  def kill_window(session, name) do
    _ = run(["kill-window", "-t", "#{session}:=#{name}"])
    :ok
  end

  @spec windows(String.t()) :: [String.t()]
  def windows(session) do
    case run(["list-windows", "-t", session, "-F", ~S(#{window_name})]) do
      {out, 0} -> String.split(out, "\n", trim: true)
      _ -> []
    end
  end

  @spec capture(String.t(), String.t()) :: String.t()
  def capture(session, name) do
    case run(["capture-pane", "-p", "-J", "-S", "-200", "-t", "#{session}:=#{name}"]) do
      {out, 0} -> out
      _ -> ""
    end
  end

  @doc "Whether a pane still has a foreground CLI below its retained wrapper. Fail closed on errors."
  def pane_process(session, name) do
    case run(["list-panes", "-t", "#{session}:=#{name}", "-F", ~S(#{pane_pid} #{pane_dead})]) do
      {out, 0} ->
        case String.split(String.trim(out)) do
          [_pid, "1"] ->
            :dead

          [pid, "0"] ->
            case System.cmd("ps", ["-axo", "pid=,ppid="], stderr_to_stdout: true) do
              {table, 0} ->
                # Only PIDs/parent PIDs: never argv or environment contents.
                children =
                  for line <- String.split(table, "\n"),
                      [child, parent] <- [String.split(line)],
                      parent == pid,
                      do: child

                {:alive, children != []}

              _ ->
                :unknown
            end

          _ ->
            :unknown
        end

      {_, _} ->
        if name in windows(session), do: :unknown, else: :missing
    end
  end

  @doc "Make `name` the current window of the session (what an attached terminal shows)."
  @spec select_window(String.t(), String.t()) :: :ok | :error
  def select_window(session, name) do
    case run(["select-window", "-t", "#{session}:=#{name}"]) do
      {_, 0} -> :ok
      _ -> :error
    end
  end

  @spec current_window(String.t()) :: String.t() | nil
  def current_window(session) do
    case run(["display-message", "-p", "-t", session, ~S(#{window_name})]) do
      {out, 0} -> String.trim(out)
      _ -> nil
    end
  end

  @spec kill_session(String.t()) :: :ok
  def kill_session(session) do
    _ = run(["kill-session", "-t", session])
    :ok
  end

  defp run(args) do
    socket =
      case Application.get_env(:aperture, :tmux_socket) do
        nil -> []
        name -> ["-L", name]
      end

    System.cmd("tmux", socket ++ args, stderr_to_stdout: true)
  end
end
