defmodule Aperture.BeadsRecoveryTest do
  use ExUnit.Case, async: false
  import ExUnit.CaptureLog

  setup do
    dir = Path.join(System.tmp_dir!(), "bd-recovery-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)

    keys = [
      :beads_dir,
      :bd_bin,
      :bd_timeout_ms,
      :bd_queue_timeout_ms,
      :bd_total_timeout_ms,
      :bd_readback_timeout_ms,
      :bd_stop_grace_ms
    ]

    original = Map.new(keys, &{&1, Application.get_env(:aperture, &1)})
    Application.put_env(:aperture, :beads_dir, dir)

    Application.put_env(
      :aperture,
      :bd_bin,
      Path.expand("../support/bin/fake-bd-delayed", __DIR__)
    )

    Application.put_env(:aperture, :bd_timeout_ms, 1_000)
    Application.put_env(:aperture, :bd_readback_timeout_ms, 1_000)
    Application.put_env(:aperture, :bd_queue_timeout_ms, 5_000)

    on_exit(fn ->
      wait_ready()

      for {k, v} <- original do
        if v == nil,
          do: Application.delete_env(:aperture, k),
          else: Application.put_env(:aperture, k, v)
      end

      File.rm_rf!(dir)
    end)

    old_level = Logger.level()
    Logger.configure(level: :info)
    on_exit(fn -> Logger.configure(level: old_level) end)
    behavior(dir, %{})
    %{dir: dir}
  end

  defp behavior(dir, config),
    do: File.write!(Path.join(dir, "behavior.json"), JSON.encode!(config))

  defp events(dir) do
    File.read!(Path.join(dir, "events.jsonl"))
    |> String.split("\n", trim: true)
    |> Enum.map(&JSON.decode!/1)
  end

  test "write + JSON + slow exit is indeterminate with readback, never a create retry", %{
    dir: dir
  } do
    behavior(dir, %{delays: %{create: 1_500}})

    log =
      capture_log([level: :info], fn ->
        assert {:indeterminate, result} =
                 Aperture.Beads.run("create", "peppy", %{"title" => "PAYLOAD-MUST-NOT-BE-LOGGED"})

        assert result["outcome"] == "indeterminate"
        assert result["task_id"] == "ap-delayed"
        assert result["retry_safe"] == false
        assert result["readback"]["status"] == "observed"
        assert result["readback"]["task"]["id"] == "ap-delayed"
      end)

    starts = Enum.filter(events(dir), &(&1["event"] == "start"))
    assert Enum.map(starts, & &1["op"]) == ["create", "show"]
    assert hd(starts)["exportAuto"] == nil
    assert List.last(starts)["exportAuto"] == "false"
    refute Enum.any?(events(dir), &(&1["event"] == "OVERLAP"))
    assert log =~ "os_pid="
    assert log =~ "elapsed_ms="
    refute log =~ "PAYLOAD-MUST-NOT-BE-LOGGED"
    refute log =~ "ap-delayed"
  end

  test "timeout without a create receipt remains unknown and never searches by title", %{dir: dir} do
    behavior(dir, %{delays: %{create: 1_500}, noOutput: true})
    assert {:indeterminate, r} = Aperture.Beads.run("create", "peppy", %{"title" => "unknown"})
    assert r["task_id"] == nil
    assert r["readback"]["status"] == "unavailable"
    assert Enum.count(events(dir), &(&1["event"] == "start")) == 1
  end

  test "append-note timeout reads the known id without appending twice", %{dir: dir} do
    behavior(dir, %{delays: %{update: 1_500}})

    assert {:indeterminate, r} =
             Aperture.Beads.run("update", "peppy", %{"id" => "ap-delayed", "notes" => "once"})

    assert r["readback"]["task"]["notes"] == "once"
    assert Enum.count(events(dir), &(&1["event"] == "start" && &1["op"] == "update")) == 1
  end

  test "normal calls and TeamTasks poll share the same serial queue", %{dir: dir} do
    behavior(dir, %{delays: %{list: 120}})
    tasks = for _ <- 1..3, do: Task.async(fn -> Aperture.Beads.run("list", "peppy", %{}) end)
    poll = Task.async(fn -> Aperture.Web.TeamTasks.refresh() end)
    for t <- tasks, do: assert({:ok, _} = Task.await(t, 5_000))
    assert :ok == Task.await(poll, 5_000)
    refute Enum.any?(events(dir), &(&1["event"] == "OVERLAP"))
    assert Enum.count(events(dir), &(&1["event"] == "start")) == 4
    assert Enum.all?(events(dir), &(&1["exportAuto"] == "false"))
  end

  test "readback timeout does not turn a write into a retryable failure", %{dir: dir} do
    behavior(dir, %{delays: %{close: 1_500, show: 1_500}})

    assert {:indeterminate, r} =
             Aperture.Beads.run("close", "peppy", %{"id" => "ap-delayed", "reason" => "done"})

    assert r["readback"]["status"] == "unavailable"
    assert r["retry_safe"] == false

    assert Enum.map(Enum.filter(events(dir), &(&1["event"] == "start")), & &1["op"]) == [
             "close",
             "show"
           ]
  end

  test "a timed out read is an error even with complete JSON, not a mutation outcome", %{dir: dir} do
    behavior(dir, %{delays: %{list: 1_500}})
    assert {:error, "bd read timed out" <> _} = Aperture.Beads.run("list", "peppy", %{})
  end

  test "a queued expired mutation never starts", %{dir: dir} do
    behavior(dir, %{delays: %{list: 180}})
    Application.put_env(:aperture, :bd_queue_timeout_ms, 30)
    reader = Task.async(fn -> Aperture.Beads.run("list", "peppy", %{}) end)
    wait_started(dir)

    assert {:error, "bd queue wait exceeded; command not started"} =
             Aperture.Beads.run("create", "peppy", %{"title" => "must not run"})

    assert {:ok, _} = Task.await(reader)
    refute File.exists?(Path.join(dir, "task.json"))
  end

  test "HTTP/MCP preserves the indeterminate outcome and readback, not generic success", %{
    dir: dir
  } do
    behavior(dir, %{delays: %{create: 1_500}})
    script = Path.join(Application.fetch_env!(:aperture, :root), "bin/aperture-mcp.mjs")

    input =
      JSON.encode!(%{
        jsonrpc: "2.0",
        id: 1,
        method: "tools/call",
        params: %{name: "create_task", arguments: %{title: "delayed"}}
      }) <> "\n"

    {out, 0} =
      System.cmd(
        "sh",
        [
          "-c",
          ~s(printf '%s' "$REQS" | node "$0" --agent peppy --hub "$1"),
          script,
          "http://127.0.0.1:#{Application.fetch_env!(:aperture, :port)}"
        ],
        env: [{"REQS", input}]
      )

    reply = JSON.decode!(String.trim(out))
    assert reply["result"]["isError"]
    result = JSON.decode!(hd(reply["result"]["content"])["text"])
    assert result["outcome"] == "indeterminate"
    assert result["readback"]["status"] == "observed"
    assert result["retry_safe"] == false
  end

  test "TERM-resistant CLI is reaped before readback and the next command", %{dir: dir} do
    behavior(dir, %{delays: %{create: 10_000}, ignoreTerm: true})
    started = System.monotonic_time(:millisecond)
    assert {:indeterminate, r} = Aperture.Beads.run("create", "peppy", %{"title" => "stubborn"})
    assert System.monotonic_time(:millisecond) - started < 4_000
    assert r["readback"]["status"] == "observed"
    refute Enum.any?(events(dir), &(&1["event"] == "OVERLAP"))
    pid = Enum.find(events(dir), &(&1["event"] == "start" && &1["op"] == "create"))["pid"]
    assert {_, 1} = System.cmd("/bin/kill", ["-0", to_string(pid)], stderr_to_stdout: true)
    assert {:ok, _} = Aperture.Beads.run("list", "peppy", %{})
  end

  test "total call deadline bounds queue plus command plus recovery", %{dir: dir} do
    behavior(dir, %{delays: %{list: 120, create: 10_000, show: 10_000}})
    Application.put_env(:aperture, :bd_total_timeout_ms, 800)
    Application.put_env(:aperture, :bd_stop_grace_ms, 50)
    Application.put_env(:aperture, :bd_queue_timeout_ms, 500)
    Application.put_env(:aperture, :bd_readback_timeout_ms, 150)
    reader = Task.async(fn -> Aperture.Beads.run("list", "peppy", %{}) end)
    wait_started(dir)
    started = System.monotonic_time(:millisecond)
    assert {:indeterminate, r} = Aperture.Beads.run("create", "peppy", %{"title" => "deadline"})
    elapsed = System.monotonic_time(:millisecond) - started
    assert elapsed < 950
    assert r["retry_safe"] == false
    Task.await(reader)
    Application.put_env(:aperture, :bd_total_timeout_ms, 99_000)
    assert Aperture.Beads.Queue.budget_ms() <= 45_000
  end

  test "exception after spawning keeps queue alive and blocked until exit" do
    owner = Process.whereis(Aperture.Beads.Queue)

    log =
      capture_log(fn ->
        reply =
          try do
            Aperture.Beads.Queue.run("create", fn ->
              p =
                Port.open({:spawn_executable, "/bin/sleep"}, [:binary, :exit_status, args: ["1"]])

              Aperture.Beads.Queue.track(p)
              raise "MUST-NOT-LOG-EXCEPTION-PAYLOAD"
            end)
          catch
            :exit, reason -> {:queue_crashed, reason}
          end

        assert {:indeterminate, r} = reply
        assert r["retry_safe"] == false
        assert Process.whereis(Aperture.Beads.Queue) == owner
        assert Aperture.Beads.Queue.health() == "blocked"
        assert {:error, "bd queue blocked" <> _} = Aperture.Beads.run("list", "peppy", %{})
        health = Plug.Test.conn(:get, "http://127.0.0.1/health") |> Aperture.Hub.Endpoint.call([])
        assert health.status == 200
        assert Plug.Conn.get_resp_header(health, "x-aperture-beads") == ["blocked"]
        assert health.resp_body =~ "blocked"
        wait_ready()
        assert {:ok, _} = Aperture.Beads.run("list", "peppy", %{})
      end)

    assert log =~ "bd queue blocked"
    assert log =~ "bd queue unblocked"
    refute log =~ "MUST-NOT-LOG-EXCEPTION-PAYLOAD"
  end

  test "child holding stdout fails closed and later unblocks", %{dir: dir} do
    behavior(dir, %{delays: %{create: 10_000}, holdStdoutMs: 2_500})
    Application.put_env(:aperture, :bd_stop_grace_ms, 50)
    assert {:indeterminate, r} = Aperture.Beads.run("create", "peppy", %{"title" => "child"})
    assert r["readback"]["reason"] == "CLI exit unconfirmed"
    assert Aperture.Beads.Queue.health() == "blocked"
    assert {:error, "bd queue blocked" <> _} = Aperture.Beads.run("list", "peppy", %{})
    wait_ready()
    behavior(dir, %{})
    assert {:ok, _} = Aperture.Beads.run("list", "peppy", %{})

    health = Plug.Test.conn(:get, "http://127.0.0.1/health") |> Aperture.Hub.Endpoint.call([])
    assert health.status == 200
    assert Plug.Conn.get_resp_header(health, "x-aperture-beads") == ["ready"]
    assert health.resp_body =~ "ready"
  end

  defp wait_ready(attempts \\ 100)
  defp wait_ready(0), do: flunk("queue did not recover")

  defp wait_ready(n) do
    if Aperture.Beads.Queue.health() == "ready",
      do: :ok,
      else:
        (
          Process.sleep(50)
          wait_ready(n - 1)
        )
  end

  test "malformed CLI JSON shape cannot crash the shared queue", %{dir: dir} do
    owner = Process.whereis(Aperture.Beads.Queue)
    behavior(dir, %{malformedList: true})
    assert {:error, _} = Aperture.Beads.run("list", "peppy", %{})
    assert Process.whereis(Aperture.Beads.Queue) == owner
    assert Aperture.Beads.Queue.health() == "ready"
    behavior(dir, %{})
    assert {:ok, _} = Aperture.Beads.run("list", "peppy", %{})
  end

  test "caller deadline returns safely even behind a stuck callback, and stale writes never start" do
    Application.put_env(:aperture, :bd_total_timeout_ms, 150)
    parent = self()

    blocker =
      Task.async(fn ->
        Aperture.Beads.Queue.run("list", fn ->
          send(parent, :callback_started)
          Process.sleep(450)
          {{:ok, []}, nil}
        end)
      end)

    assert_receive :callback_started
    started = System.monotonic_time(:millisecond)

    assert {:indeterminate, r} =
             Aperture.Beads.Queue.run("create", fn ->
               send(parent, :abandoned_write_started)
               {{:ok, []}, nil}
             end)

    assert System.monotonic_time(:millisecond) - started < 300
    assert r["retry_safe"] == false
    assert {:error, _} = Task.await(blocker)
    wait_ready()
    refute_receive :abandoned_write_started
  end

  test "MCP has a transport/body deadline with an explicit no-retry result" do
    script = Path.expand("../support/beads_transport_test.mjs", __DIR__)
    {out, code} = System.cmd("node", ["--test", script], stderr_to_stdout: true)
    assert code == 0, out
  end

  defp wait_started(dir, attempts \\ 100)
  defp wait_started(_dir, 0), do: flunk("CLI did not start")

  defp wait_started(dir, attempts) do
    if File.exists?(Path.join(dir, "events.jsonl")) do
      :ok
    else
      Process.sleep(10)
      wait_started(dir, attempts - 1)
    end
  end
end
