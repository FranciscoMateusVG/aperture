defmodule Aperture.Beads.Command do
  @moduledoc false
  require Logger
  alias Aperture.Beads.Queue

  # Port ownership stays with the queue. exec keeps the OS pid the actual CLI,
  # not a shell wrapper. Separate stderr cannot corrupt stdout JSON.
  def run(args, actor, op) do
    started = now()

    dir =
      Path.join(
        System.tmp_dir!(),
        "aperture-bd-" <> Base.url_encode64(:crypto.strong_rand_bytes(18))
      )

    File.mkdir!(dir)
    File.chmod!(dir, 0o700)
    stderr = Path.join(dir, "stderr")

    env = [
      {"BEADS_DIR", Application.fetch_env!(:aperture, :beads_dir)},
      {"BD_ACTOR", actor},
      {"APERTURE_BD_STDERR", stderr}
    ]

    env = if hd(args) in ~w(list ready show), do: [{"BD_EXPORT_AUTO", "false"} | env], else: env
    bin = Application.fetch_env!(:aperture, :bd_bin)

    timeout =
      if op == "readback",
        do: Queue.bounded(:bd_readback_timeout_ms, 5_000),
        else: Queue.bounded(:bd_timeout_ms, 28_000)

    # Reserve both termination grace periods within the absolute call deadline.
    deadline = min(started + timeout, Queue.deadline() - 2 * grace())

    try do
      if now() >= deadline, do: throw(:bd_deadline)

      port =
        open_port([
          :binary,
          :exit_status,
          :use_stdio,
          args: ["-c", ~s(exec "$0" "$@" 2>"$APERTURE_BD_STDERR"), bin | args],
          env: Enum.map(env, fn {k, v} -> {String.to_charlist(k), String.to_charlist(v)} end)
        ])

      Queue.track(port)

      pid =
        case Port.info(port, :os_pid) do
          {:os_pid, pid} -> pid
          nil -> nil
        end

      result = collect(port, deadline, [])

      {kind, code, chunks, blocked} =
        case result do
          {:exit, code, chunks} ->
            {:exit, code, chunks, nil}

          {:timeout, chunks} ->
            # Never let recovery or the next write overlap a timed-out CLI.
            # Killing a CLI does NOT prove its transaction rolled back.
            stop(port, pid, chunks)
        end

      elapsed = now() - started

      Logger.info(
        "bd command op=#{op} os_pid=#{pid} elapsed_ms=#{elapsed} outcome=#{kind} exit_code=#{inspect(code)}"
      )

      err =
        case File.read(stderr) do
          {:ok, data} -> data
          _ -> ""
        end

      %{
        kind: kind,
        code: code,
        out: chunks |> Enum.reverse() |> IO.iodata_to_binary(),
        err: err,
        blocked: blocked
      }
    catch
      :bd_deadline ->
        %{kind: :timeout, code: nil, out: "", err: "", blocked: nil}

      :bd_launch_error ->
        Logger.warning("bd command op=#{op} elapsed_ms=#{now() - started} outcome=launch_error")
        %{kind: :launch_error, code: nil, out: "", err: "", blocked: nil}
    after
      File.rm_rf(dir)
    end
  end

  defp open_port(opts) do
    Port.open({:spawn_executable, System.find_executable("sh")}, opts)
  rescue
    _ -> throw(:bd_launch_error)
  end

  defp collect(port, deadline, chunks) do
    if now() >= deadline do
      {:timeout, chunks}
    else
      receive do
        {^port, {:data, data}} ->
          collect(port, deadline, [data | chunks])

        {^port, {:exit_status, code}} ->
          Queue.confirm_exit(port)
          {:exit, code, chunks}
      after
        max(0, deadline - now()) -> {:timeout, chunks}
      end
    end
  end

  defp stop(port, pid, chunks) do
    signal(port, pid, "-TERM")

    case collect(port, now() + grace(), chunks) do
      {:exit, code, out} ->
        {:timeout, code, out, nil}

      {:timeout, out} ->
        signal(port, pid, "-KILL")

        case collect(port, now() + grace(), out) do
          {:exit, code, data} -> {:timeout, code, data, nil}
          {:timeout, data} -> {:timeout, nil, data, port}
        end
    end
  end

  defp signal(port, pid, signal) do
    # Fixed signal and integer OS pid only; no caller-supplied command text.
    if is_integer(pid) and Port.info(port, :os_pid) == {:os_pid, pid} do
      System.cmd("/bin/kill", [signal, Integer.to_string(pid)], stderr_to_stdout: true)
    end

    :ok
  end

  defp grace, do: Queue.bounded(:bd_stop_grace_ms, 1_000)

  defp now, do: System.monotonic_time(:millisecond)
end
