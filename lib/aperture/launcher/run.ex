defmodule Aperture.Launcher.Run do
  @moduledoc """
  Per-invocation, content-free lifecycle receipts. Old runs are retained.
  A receipt is evidence of exit, not evidence of liveness: tmux + the OS
  process tree are checked before Start is allowed to replace a window.
  """

  def prepare(root, window) do
    id = Base.encode16(:crypto.strong_rand_bytes(16), case: :lower)
    dir = Path.join([root, "lifecycle", window, id])
    File.mkdir_p!(dir)
    File.chmod!(dir, 0o700)
    atomic(Path.join(root, window <> ".run"), id)
    state(root, window, :starting)
    dir
  end

  def info(root, window) do
    case directory(root, window) do
      nil ->
        %{state: :offline, exit: nil, run_id: nil}

      dir ->
        exit = json(Path.join(dir, "exit.json"))

        status =
          case File.read(Path.join(dir, "state")) do
            {:ok, "started"} -> if(exit, do: :exited, else: :started)
            {:ok, "startup_failed"} -> :startup_failed
            {:ok, "starting"} -> if(exit, do: :startup_failed, else: :starting)
            _ -> :offline
          end

        %{state: status, exit: exit, run_id: Path.basename(dir)}
    end
  end

  def state(root, window, state) when state in [:starting, :started, :startup_failed, :stopped] do
    if dir = directory(root, window), do: atomic(Path.join(dir, "state"), Atom.to_string(state))
    :ok
  end

  def process(root, session, window) do
    case Aperture.Tmux.pane_process(session, window) do
      :missing ->
        :missing

      :dead ->
        :exited

      {:alive, true} ->
        :live

      {:alive, false} ->
        case info(root, window) do
          %{exit: exit} when is_map(exit) -> :exited
          %{run_id: id} when is_binary(id) -> :starting
          _ -> :unknown
        end

      :unknown ->
        :unknown
    end
  end

  defp directory(root, window) do
    case File.read(Path.join(root, window <> ".run")) do
      {:ok, id} ->
        if Regex.match?(~r/\A[0-9a-f]{32}\z/, id),
          do: Path.join([root, "lifecycle", window, id])

      _ ->
        nil
    end
  end

  defp json(file) do
    with {:ok, text} <- File.read(file),
         {:ok, value} when is_map(value) <- JSON.decode(text),
         do: value,
         else: (_ -> nil)
  end

  defp atomic(file, text) do
    temp = file <> ".tmp"
    File.write!(temp, text)
    File.rename!(temp, file)
  end
end
