defmodule Aperture.NodeClient do
  @moduledoc """
  Runs one of the real `bin/*.mjs` clients as an OS process and collects its
  stdout lines, so tests exercise the exact code a Claude agent will run.
  """

  # Resolve the real node binary. Version managers (Volta, asdf) put a shim on
  # PATH that execs node as a *child* process; killing the shim's pid would
  # leave the real client alive and still connected to the hub.
  def node_path do
    case System.find_executable("node") do
      nil ->
        nil

      shim ->
        case System.cmd(shim, ["-p", "process.execPath"], stderr_to_stdout: true) do
          {path, 0} -> String.trim(path)
          _ -> shim
        end
    end
  end

  def start(script, args) do
    path = Path.expand("../../bin/#{script}", __DIR__)

    # via sh so the client's stderr logs stay out of the test output
    port =
      Port.open({:spawn_executable, "/bin/sh"}, [
        :binary,
        :exit_status,
        {:line, 65_536},
        args: ["-c", ~s(exec "$0" "$@" 2>/dev/null), node_path(), path | args]
      ])

    {:ok, lines} = Agent.start_link(fn -> [] end)
    collector = spawn_link(fn -> collect(port, lines) end)
    Port.connect(port, collector)
    %{port: port, lines: lines, os_pid: Port.info(port)[:os_pid]}
  end

  def write(%{port: port}, data), do: Port.command(port, data)

  def lines(%{lines: lines}), do: Agent.get(lines, &Enum.reverse/1)

  def kill(%{os_pid: pid}), do: System.cmd("kill", ["-9", to_string(pid)], stderr_to_stdout: true)

  def wait_line(client, pred, timeout \\ 5_000) do
    Aperture.FakeAgent.wait_until(
      fn -> Enum.any?(lines(client), pred) end,
      "no matching line; got #{inspect(lines(client))}",
      timeout
    )

    Enum.find(lines(client), pred)
  end

  defp collect(port, lines) do
    receive do
      {^port, {:data, {:eol, line}}} ->
        Agent.update(lines, &[line | &1])
        collect(port, lines)

      {^port, {:data, {:noeol, _}}} ->
        collect(port, lines)

      {^port, {:exit_status, _}} ->
        :ok
    end
  end
end
