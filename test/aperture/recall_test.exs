defmodule Aperture.RecallTest do
  @moduledoc """
  v3's recall memory, read-only, against a fixture shaped like
  ~/.aperture/run/memory-index.json (v3's MemoryIndex cache).
  """
  use ExUnit.Case, async: false

  @cadence "incluir-prod-backup-cadence-minio-is-weekly-sundays"

  defp entry(key, body, meta \\ %{}, extra \\ %{}) do
    Map.merge(
      %{
        "key" => key,
        "body" => body,
        "meta" => meta,
        "secret" => false,
        "supersededBy" => nil,
        "redacted" => false
      },
      extra
    )
  end

  defp write_index(entries) do
    path = Application.fetch_env!(:aperture, :memory_index)

    index = %{
      "builtAt" => "2026-09-07T00:00:00Z",
      "entries" => entries,
      "lines" => for(e <- entries, do: %{"key" => e["key"], "gist" => "gist of #{e["key"]}"})
    }

    File.write!(path, JSON.encode!(index))
    # force a new mtime so the cache reloads even within the same second
    File.touch!(path, System.os_time(:second) + System.unique_integer([:positive]))
    path
  end

  setup do
    write_index([
      entry(
        @cadence,
        "INCLUIR PROD BACKUP CADENCE — MinIO is WEEKLY (Sundays), NOT daily.\n  20 6 * * 0   incluir-minio-backup.sh",
        %{"tags" => ["backup", "minio"], "project" => "incluir", "updated" => "2026-08-14"}
      ),
      entry("incluir-backup-daily", "MinIO backups run daily.", %{"tags" => ["backup"]}, %{
        "supersededBy" => @cadence
      }),
      entry(
        "dokploy-admin-credentials",
        "token=[REDACTED] for the MinIO backup admin",
        %{"tags" => ["secret"]},
        %{"secret" => true}
      ),
      entry("native-workflow-first", "Use native tooling before building a backup script.", %{
        "standing" => true
      }),
      entry("sqlite-wal-fake-backup-trap", String.duplicate("copy the -wal file too. ", 600), %{
        "tags" => ["backup"]
      })
    ])

    :ok
  end

  defp post(path, body) do
    port = Application.fetch_env!(:aperture, :port)
    {:ok, conn} = Mint.HTTP.connect(:http, "127.0.0.1", port, mode: :passive)

    {:ok, conn, ref} =
      Mint.HTTP.request(
        conn,
        "POST",
        path,
        [{"content-type", "application/json"}],
        JSON.encode!(body)
      )

    {status, data} = recv(conn, ref, nil, "")
    Mint.HTTP.close(conn)
    {status, JSON.decode!(data)}
  end

  defp recv(conn, ref, status, acc) do
    {:ok, conn, responses} = Mint.HTTP.recv(conn, 0, 5_000)

    {status, acc, done} =
      Enum.reduce(responses, {status, acc, false}, fn
        {:status, ^ref, s}, {_, a, d} -> {s, a, d}
        {:data, ^ref, d}, {s, a, done} -> {s, a <> d, done}
        {:done, ^ref}, {s, a, _} -> {s, a, true}
        _, x -> x
      end)

    if done, do: {status, acc}, else: recv(conn, ref, status, acc)
  end

  test "search finds the banked lesson; secret and superseded memories never appear" do
    assert {200, %{"result" => [top | _] = items}} =
             post("/recall", %{from: "peppy", query: "Incluir MinIO backup cadence"})

    assert top["key"] == @cadence
    assert top["gist"] == "gist of #{@cadence}"

    keys = Enum.map(items, & &1["key"])
    refute "dokploy-admin-credentials" in keys
    refute "incluir-backup-daily" in keys
  end

  test "recall_full returns the text; an exact key ranks first" do
    assert {200, %{"result" => [%{"key" => @cadence} | _]}} =
             post("/recall", %{from: "peppy", query: @cadence})

    assert {200, %{"result" => m}} = post("/recall/full", %{from: "peppy", key: @cadence})
    assert m["body"] =~ "20 6 * * 0"
    assert m["updated"] == "2026-08-14"
    refute m["truncated"]
  end

  test "secret and superseded memories cannot be read even by key" do
    assert {422, %{"error" => "no such memory"}} =
             post("/recall/full", %{from: "peppy", key: "dokploy-admin-credentials"})

    assert {422, %{"error" => "no such memory"}} =
             post("/recall/full", %{from: "peppy", key: "incluir-backup-daily"})
  end

  test "long memories are capped" do
    assert {200, %{"result" => m}} =
             post("/recall/full", %{from: "peppy", key: "sqlite-wal-fake-backup-trap"})

    assert m["truncated"]
    assert byte_size(m["body"]) <= 8 * 1024 + 20
  end

  test "picks up a changed index without a restart" do
    write_index([
      entry("tailscale-ssh-hang", "xerox SSH hangs via the alias; use the tailnet IP")
    ])

    assert {200, %{"result" => [%{"key" => "tailscale-ssh-hang"}]}} =
             post("/recall", %{from: "peppy", query: "ssh hangs"})
  end

  test "a missing index is a clear error, not a crash" do
    File.rm!(Application.fetch_env!(:aperture, :memory_index))

    assert {422, %{"error" => "v3 memory index not found at " <> _}} =
             post("/recall", %{from: "peppy", query: "anything"})
  end

  @tag skip: if(System.find_executable("node"), do: false, else: "needs node")
  test "agents reach it through the aperture MCP recall tool" do
    port = Application.fetch_env!(:aperture, :port)
    script = Path.join(Application.fetch_env!(:aperture, :root), "bin/aperture-mcp.mjs")

    reqs =
      Enum.map_join(
        [
          %{
            jsonrpc: "2.0",
            id: 1,
            method: "initialize",
            params: %{protocolVersion: "2025-06-18"}
          },
          %{
            jsonrpc: "2.0",
            id: 2,
            method: "tools/call",
            params: %{name: "recall", arguments: %{query: "minio backup cadence"}}
          }
        ],
        "",
        &(JSON.encode!(&1) <> "\n")
      )

    {out, 0} =
      System.cmd(
        "sh",
        [
          "-c",
          ~s(printf '%s' "$REQS" | node "$0" --agent peppy --hub "$1"),
          script,
          "http://127.0.0.1:#{port}"
        ],
        env: [{"REQS", reqs}]
      )

    reply =
      out
      |> String.split("\n", trim: true)
      |> Enum.map(&JSON.decode!/1)
      |> Enum.find(&(&1["id"] == 2))

    assert %{"result" => %{"content" => [%{"text" => text}]}} = reply
    assert [%{"key" => @cadence} | _] = JSON.decode!(text)
  end
end
