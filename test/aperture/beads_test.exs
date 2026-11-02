defmodule Aperture.BeadsTest do
  @moduledoc """
  BEADS through the hub, with a fake `bd` (test/support/bin/fake-bd) that
  records every call. The first test drives the real aperture-mcp script.
  """
  use ExUnit.Case, async: false

  defp port, do: Application.fetch_env!(:aperture, :port)

  defp last_call do
    Application.fetch_env!(:aperture, :beads_dir)
    |> Path.join("calls.jsonl")
    |> File.read!()
    |> String.split("\n", trim: true)
    |> List.last()
    |> JSON.decode!()
  end

  defp beads(op, body) do
    {:ok, conn} = Mint.HTTP.connect(:http, "127.0.0.1", port(), mode: :passive)

    {:ok, conn, ref} =
      Mint.HTTP.request(
        conn,
        "POST",
        "/beads/#{op}",
        [{"content-type", "application/json"}],
        JSON.encode!(body)
      )

    {status, data} = recv(conn, ref, nil, "")
    {status, JSON.decode!(data)}
  end

  defp recv(conn, ref, status, acc) do
    {:ok, conn, responses} = Mint.HTTP.recv(conn, 0, 10_000)

    {status, acc, done} =
      Enum.reduce(responses, {status, acc, false}, fn
        {:status, ^ref, s}, {_, a, d} -> {s, a, d}
        {:data, ^ref, d}, {s, a, done} -> {s, a <> d, done}
        {:done, ^ref}, {s, a, _} -> {s, a, true}
        _, x -> x
      end)

    if done do
      Mint.HTTP.close(conn)
      {status, acc}
    else
      recv(conn, ref, status, acc)
    end
  end

  # Pipe JSON-RPC requests into the real aperture-mcp and collect the replies.
  defp run_mcp(agent, requests) do
    script = Path.join(Application.fetch_env!(:aperture, :root), "bin/aperture-mcp.mjs")
    input = Enum.map_join(requests, "", &(JSON.encode!(&1) <> "\n"))

    {out, 0} =
      System.cmd(
        "sh",
        [
          "-c",
          ~s(printf '%s' "$REQS" | node "$0" --agent "$1" --hub "$2"),
          script,
          agent,
          "http://127.0.0.1:#{port()}"
        ],
        env: [{"REQS", input}]
      )

    out |> String.split("\n", trim: true) |> Enum.map(&JSON.decode!/1) |> Map.new(&{&1["id"], &1})
  end

  @tag skip: if(System.find_executable("node"), do: false, else: "needs node")
  test "an agent's create_task runs bd on the hub as that agent; text is taken literally" do
    replies =
      run_mcp("peppy", [
        %{jsonrpc: "2.0", id: 1, method: "initialize", params: %{protocolVersion: "2025-06-18"}},
        %{
          jsonrpc: "2.0",
          id: 2,
          method: "tools/call",
          params: %{
            name: "create_task",
            arguments: %{
              title: "--Ship the launcher",
              description: "-x looks like a flag",
              priority: 1,
              labels: ["project:aperture", "bad label!"]
            }
          }
        }
      ])

    assert %{"result" => %{"content" => [%{"text" => text}]}} = replies[2]
    # bd's stderr warning does not leak into the result; the title is kept as written
    assert %{"id" => "ap-" <> _, "title" => "--Ship the launcher"} = JSON.decode!(text)

    assert last_call() == %{
             "actor" => "peppy",
             "argv" => [
               "create",
               "--title=--Ship the launcher",
               "-p",
               "1",
               "--json",
               "--description=-x looks like a flag",
               "-l",
               "project:aperture"
             ]
           }
  end

  test "list returns summaries only; ready lists unblocked work" do
    assert {200, %{"result" => [first, _]}} = beads("list", %{from: "glados", ready: true})
    assert Map.keys(first) |> Enum.sort() == ~w(assignee id labels priority status title)
    assert last_call()["argv"] == ["ready", "--json", "-n", "50"]

    assert {200, _} =
             beads("list", %{from: "glados", status: "in_progress", assignee: "peppy", limit: 5})

    assert last_call()["argv"] == [
             "list",
             "--json",
             "--status",
             "in_progress",
             "--assignee",
             "peppy",
             "-n",
             "5"
           ]
  end

  test "show returns the full task with a capped description" do
    assert {200, %{"result" => task}} = beads("show", %{from: "wheatley", id: "ap-7"})
    assert task["acceptance"] == "it works"
    assert String.ends_with?(task["description"], "[truncated]")
    assert String.length(task["description"]) < 4_100
  end

  test "claim assigns the task to the calling agent; update appends notes; close needs a reason" do
    assert {200, %{"result" => %{"status" => "in_progress", "assignee" => "wheatley"}}} =
             beads("claim", %{from: "wheatley", id: "ap-7"})

    assert last_call() == %{
             "actor" => "wheatley",
             "argv" => [
               "update",
               "ap-7",
               "--status",
               "in_progress",
               "--assignee",
               "wheatley",
               "--json"
             ]
           }

    assert {200, _} =
             beads("update", %{
               from: "wheatley",
               id: "ap-7",
               notes: "-half done",
               add_labels: ["needs:review"]
             })

    assert last_call()["argv"] ==
             [
               "update",
               "ap-7",
               "--append-notes=-half done",
               "--add-label",
               "needs:review",
               "--json"
             ]

    assert {422, %{"error" => "nothing to update"}} =
             beads("update", %{from: "wheatley", id: "ap-7"})

    assert {422, %{"error" => "reason is required"}} =
             beads("close", %{from: "wheatley", id: "ap-7"})

    assert {200, %{"result" => %{"status" => "closed"}}} =
             beads("close", %{from: "wheatley", id: "ap-7", reason: "PR opened"})

    assert last_call()["argv"] == ["close", "ap-7", "--reason=PR opened", "--json"]
  end

  test "bad input never reaches bd; bd failures come back as errors" do
    before = last_call_or_nil()

    assert {422, %{"error" => "invalid task id"}} =
             beads("show", %{from: "glados", id: "../etc/passwd"})

    assert {422, %{"error" => "invalid agent"}} = beads("show", %{from: "not a name", id: "ap-1"})

    assert {422, %{"error" => "unknown operation"}} =
             beads("delete", %{from: "glados", id: "ap-1"})

    assert {422, %{"error" => "priority must be 0-4"}} =
             beads("create", %{from: "glados", title: "x", priority: 9})

    assert last_call_or_nil() == before

    # on failure, bd's stderr is what explains it
    assert {422, %{"error" => error}} = beads("create", %{from: "glados", title: "FAIL"})
    assert error =~ "bd exited 1:"
    assert error =~ "Error: database is locked"
  end

  defp last_call_or_nil do
    last_call()
  rescue
    _ -> nil
  end
end
