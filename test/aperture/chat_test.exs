defmodule Aperture.ChatTest do
  @moduledoc """
  The operator's chat backend: conversations, unread state, uploads, delivery
  to agents (real sockets, real Node clients) and live PubSub events.
  """
  use ExUnit.Case, async: false

  alias Aperture.{AgentFixtures, Chat, Codex, FakeAgent, FakeCodex, NodeClient}
  alias Aperture.Chat.Uploads

  @png <<0x89, "PNG", 0x0D, 0x0A, 0x1A, 0x0A, "rest of a png">>
  @jpeg <<0xFF, 0xD8, 0xFF, 0xE0, "rest of a jpeg">>
  @gif "GIF89a rest of a gif"
  @webp "RIFF" <> <<20, 0, 0, 0>> <> "WEBPVP8 rest of a webp"

  setup context do
    suffix = "#{context.line}-#{System.unique_integer([:positive])}"
    port = Application.fetch_env!(:aperture, :port)
    :ok = Chat.subscribe()

    {:ok,
     a: "ca-#{suffix}",
     ws: "ws://127.0.0.1:#{port}/inbox",
     http: "http://127.0.0.1:#{port}",
     port: port}
  end

  # -- context --------------------------------------------------------------------

  test "a conversation holds both directions, in order, with unread state", %{a: a} do
    fa = FakeAgent.start(a)

    assert {:ok, %{dir: :out, agent: ^a, body: "hello agent", read: true} = out} =
             Chat.send(a, "hello agent")

    assert_receive {:chat_message, ^out}
    assert FakeAgent.wait_for(fa, 1) == ["hello agent"]

    FakeAgent.send_message(fa, "operator", "hello operator")
    assert_receive {:chat_message, %{dir: :in, agent: ^a, body: "hello operator", read: false}}

    assert [%{dir: :out, body: "hello agent"}, %{dir: :in, body: "hello operator"}] =
             Chat.history(a)

    # the agent's ack shows up as delivered
    FakeAgent.wait_until(fn -> hd(Chat.history(a)).delivered end, "never delivered")

    assert Chat.unread_counts()[a] == 1
    assert Chat.unread_total() >= 1

    assert :ok = Chat.mark_read(a)
    assert_receive {:chat_read, ^a}
    assert Chat.unread_counts()[a] == nil
    assert [_, %{read: true}] = Chat.history(a)

    # nothing left to read: no event
    assert :ok = Chat.mark_read(a)
    refute_receive {:chat_read, ^a}, 100
  end

  test "resetting an empty conversation leaves later messages visible", %{a: a} do
    assert :ok = Chat.reset(a)
    assert Chat.history(a) == []
    assert {:error, _} = Chat.reset("operator")
    assert {:error, _} = Chat.reset("invalid/name")
    {:ok, msg} = Chat.send(a, "first after empty reset")
    assert [%{id: id}] = Chat.history(a)
    assert id == msg.id
  end

  test "archive persists a boundary without changing messages, attachments or delivery", %{a: a} do
    a = AgentFixtures.define(a)
    {:ok, image} = Chat.store_upload_binary(@png)
    {:ok, out} = Chat.send(a, "pending outgoing", [image.id])
    {:ok, incoming} = Aperture.Store.insert(a, "operator", "unread archived")
    {:ok, unrelated} = Aperture.Store.insert("peer", a, "agent to agent")
    original = Aperture.Store.get(incoming.id)
    assert :ok = Chat.reset(a)
    assert_receive {:chat_reset, ^a}
    assert Chat.history(a) == []
    assert Chat.history(a, 200, before: out.id + 1) == []
    assert Chat.unread_counts()[a] == nil
    assert Enum.find(Chat.conversations(), &(&1.agent == a)).last == nil
    assert Aperture.Store.get(incoming.id) == original
    assert Aperture.Store.get(out.id).attachments == [Map.take(image, [:id, :path, :mime, :size])]
    assert File.read!(image.path) == @png
    assert Enum.map(Aperture.Store.unacked(a), & &1.id) == [out.id, unrelated.id]

    # Independent SQLite connection proves this is durable, not LiveView/GenServer state.
    {:ok, db} = Exqlite.Sqlite3.open(Application.fetch_env!(:aperture, :db_path))

    {:ok, stmt} =
      Exqlite.Sqlite3.prepare(db, "SELECT through_id FROM chat_archives WHERE peer = ?1")

    :ok = Exqlite.Sqlite3.bind(stmt, [a])
    {:ok, [[cutoff]]} = Exqlite.Sqlite3.fetch_all(db, stmt)
    assert cutoff >= incoming.id
    Exqlite.Sqlite3.release(db, stmt)
    Exqlite.Sqlite3.close(db)

    {:ok, fresh} = Aperture.Store.insert(a, "operator", "fresh unread")
    assert [%{id: id}] = Chat.history(a)
    assert id == fresh.id
    assert Chat.unread_counts()[a] == 1
    assert :ok = Chat.reset(a)
    assert Chat.history(a) == []
    assert Aperture.Store.get(fresh.id).body == "fresh unread"
  end

  test "mark_read up_to: only what the operator has seen is read", %{a: a} do
    fa = FakeAgent.start(a)
    for i <- 1..3, do: FakeAgent.send_message(fa, "operator", "r#{i}")
    FakeAgent.wait_until(fn -> length(Chat.history(a)) == 3 end, "replies not stored")
    [r1, r2, _r3] = Chat.history(a)

    assert :ok = Chat.mark_read(a, up_to: r2.id)
    assert_receive {:chat_read, ^a}
    assert Chat.unread_counts()[a] == 1
    assert [%{read: true}, %{read: true}, %{read: false}] = Chat.history(a)

    # older than everything unread: nothing changes, no event
    assert :ok = Chat.mark_read(a, up_to: r1.id)
    refute_receive {:chat_read, ^a}, 100
    assert Chat.unread_counts()[a] == 1
  end

  test "mark_read ids: exactly the messages shown, never older backlog", %{a: a} do
    fa = FakeAgent.start(a)
    for i <- 1..4, do: FakeAgent.send_message(fa, "operator", "r#{i}")
    FakeAgent.wait_until(fn -> length(Chat.history(a)) == 4 end, "replies not stored")

    # the page loaded only the last two
    [r1, r2, r3, r4] = Chat.history(a)
    assert :ok = Chat.mark_read(a, ids: [r3.id, r4.id])
    assert_receive {:chat_read, ^a}
    assert Enum.map(Chat.history(a), & &1.read) == [false, false, true, true]

    # ids of other agents' or outgoing messages are ignored
    {:ok, out} = Chat.send(a, "mine")
    assert :ok = Chat.mark_read("someone-else", ids: [r1.id])
    assert :ok = Chat.mark_read(a, ids: [out.id, r2.id])
    assert Enum.map(Chat.history(a), & &1.read) == [false, true, true, true, true]
    assert :ok = Chat.mark_read(a, ids: [])
  end

  test "history pages backwards with limit and before", %{a: a} do
    for i <- 1..5, do: {:ok, _} = Chat.send(a, "m#{i}")

    assert [%{body: "m4"}, %{body: "m5"}] = Chat.history(a, 2)
    [%{id: m4} | _] = Chat.history(a, 2)
    assert [%{body: "m2"}, %{body: "m3"}] = Chat.history(a, 2, before: m4)
  end

  test "conversations list every defined agent, latest activity first" do
    quiet = AgentFixtures.define(AgentFixtures.unique("cq"))
    busy = AgentFixtures.define(AgentFixtures.unique("cb"))
    fb = FakeAgent.start(busy)
    FakeAgent.send_message(fb, "operator", "ping")
    assert_receive {:chat_message, %{agent: ^busy}}

    convs = Chat.conversations()
    assert %{unread: 1, last: %{body: "ping", dir: :in}} = Enum.find(convs, &(&1.agent == busy))
    assert %{unread: 0, last: nil} = Enum.find(convs, &(&1.agent == quiet))
    assert hd(convs).agent == busy
  end

  test "send rejects bad targets and empty messages" do
    assert {:error, _} = Chat.send("operator", "hi")
    assert {:error, _} = Chat.send("../etc", "hi")
    assert {:error, "empty message"} = Chat.send("someone", "   ")
    assert {:error, "unknown upload" <> _} = Chat.send("someone", "hi", ["nope.png"])
  end

  # -- uploads --------------------------------------------------------------------

  test "png, jpeg, webp and gif are stored under a generated name" do
    for {data, ext, mime} <- [
          {@png, "png", "image/png"},
          {@jpeg, "jpg", "image/jpeg"},
          {@webp, "webp", "image/webp"},
          {@gif, "gif", "image/gif"}
        ] do
      assert {:ok, %{id: id, path: path, mime: ^mime, size: size, url: url}} =
               Chat.store_upload_binary(data)

      assert id =~ ~r/\A[0-9a-f]{32}\.#{ext}\z/
      assert path == Path.join(Uploads.dir(), id)
      assert Path.type(path) == :absolute
      assert url == "/uploads/" <> id
      assert size == byte_size(data)
      assert File.read!(path) == data
    end
  end

  test "the type comes from magic bytes, never the file name" do
    tmp = Path.join(System.tmp_dir!(), "evil-#{System.unique_integer([:positive])}.png")
    on_exit(fn -> File.rm(tmp) end)

    File.write!(tmp, "<svg onload=alert(1)>")
    assert {:error, :bad_type} = Chat.store_upload(tmp)

    File.write!(tmp, "")
    assert {:error, :bad_type} = Chat.store_upload(tmp)

    assert {:error, :bad_type} = Chat.store_upload_binary("GIF87b nope")
    assert {:error, :bad_type} = Chat.store_upload_binary("RIFF....AVI ")
    assert {:error, :bad_type} = Chat.store_upload("/nonexistent/x.png")
    assert {:error, :bad_type} = Chat.store_upload(System.tmp_dir!())

    File.write!(tmp, @jpeg)
    assert {:ok, %{mime: "image/jpeg", id: id}} = Chat.store_upload(tmp)
    refute id =~ "evil"
  end

  test "files over 10MB are refused" do
    tmp = Path.join(System.tmp_dir!(), "big-#{System.unique_integer([:positive])}.png")
    on_exit(fn -> File.rm(tmp) end)
    big = @png <> :binary.copy(<<0>>, Uploads.max_bytes() - byte_size(@png) + 1)

    File.write!(tmp, big)
    assert {:error, :too_large} = Chat.store_upload(tmp)
    assert {:error, :too_large} = Chat.store_upload_binary(big)

    exact = binary_part(big, 0, Uploads.max_bytes())
    assert {:ok, %{size: size}} = Chat.store_upload_binary(exact)
    assert size == Uploads.max_bytes()
  end

  test "upload ids that are not generated names never resolve" do
    outside = Path.join(Path.dirname(Uploads.dir()), "secret.png")
    File.write!(outside, @png)
    on_exit(fn -> File.rm(outside) end)

    for id <- [
          "../secret.png",
          "..%2Fsecret.png",
          "/etc/passwd",
          "secret.png",
          String.duplicate("a", 32) <> ".svg",
          String.duplicate("A", 32) <> ".png",
          String.duplicate("a", 32) <> ".png/../../secret.png",
          String.duplicate("a", 32) <> ".png\n",
          nil
        ] do
      assert Uploads.path(id) == :error, inspect(id)
      assert Uploads.fetch(id) == :error
    end
  end

  test "uploads are served by the hub, locally only, with their real type", %{port: port} do
    {:ok, %{url: url}} = Chat.store_upload_binary(@png)

    assert {200, headers, @png} = http_get(port, url)
    assert {"content-type", "image/png"} in headers
    assert {"x-content-type-options", "nosniff"} in headers

    assert {404, _, _} = http_get(port, "/uploads/" <> String.duplicate("0", 32) <> ".png")
    assert {404, _, _} = http_get(port, "/uploads/..%2Fsecret.png")
    assert {404, _, _} = http_get(port, "/uploads/../aperture.db")
    assert {403, _, _} = http_get(port, url, [{"host", "evil.example:#{port}"}])
    assert {403, _, _} = http_get(port, url, [{"origin", "https://evil.example"}])
  end

  # -- delivery ---------------------------------------------------------------------

  test "a Claude agent's real inbox gets the image's absolute path, and the file is there",
       %{a: a, ws: ws} do
    inbox = NodeClient.start("aperture-inbox.mjs", ["--agent", a, "--url", ws])
    on_exit(fn -> NodeClient.kill(inbox) end)
    wait_connected(a)

    {:ok, img} = Chat.store_upload_binary(@png)
    assert {:ok, msg} = Chat.send(a, "look at this", [img.id])
    assert msg.body == "look at this"
    assert [%{id: id, path: path, url: "/uploads/" <> id}] = msg.attachments
    assert id == img.id

    line = NodeClient.wait_line(inbox, &String.contains?(&1, "from operator"))
    assert line =~ "look at this\\n\\n[image attached: #{img.path}]"
    [_, seen_path] = Regex.run(~r/\[image attached: ([^\]]+)\]/, line)
    assert File.read!(seen_path) == @png

    # the stored history shows what the operator typed, with the image
    assert [%{body: "look at this", attachments: [%{id: ^id, path: ^path}]}] = Chat.history(a)
  end

  test "an image-only message is just the paths", %{a: a} do
    fa = FakeAgent.start(a)
    {:ok, i1} = Chat.store_upload_binary(@png)
    {:ok, i2} = Chat.store_upload_binary(@gif)
    assert {:ok, %{body: ""}} = Chat.send(a, "", [i1.id, i2.id])

    assert FakeAgent.wait_for(fa, 1) == [
             "[image attached: #{i1.path}]\n[image attached: #{i2.path}]"
           ]

    assert [%{body: "", attachments: [%{id: id1}, %{id: id2}]}] = Chat.history(a)
    assert {id1, id2} == {i1.id, i2.id}
  end

  test "a Codex agent gets the path in the text and the image as a localImage input" do
    n = System.unique_integer([:positive])
    socket = Path.join(System.tmp_dir!(), "apch-#{n}.sock")
    codex = "chat-codex-#{n}"

    on_exit(fn ->
      Codex.stop_bridge(codex)
      File.rm(socket)
    end)

    fake = FakeCodex.start(socket)
    {:ok, _} = Codex.start_bridge(codex, socket)
    wait_connected(codex)

    {:ok, img} = Chat.store_upload_binary(@webp)
    {:ok, _} = Chat.send(codex, "what is this?", [img.id])

    FakeAgent.wait_until(
      fn -> Enum.any?(FakeCodex.injects(fake), fn {_, t, _} -> t =~ "what is this?" end) end,
      "codex never got the message"
    )

    {_, text, params} =
      Enum.find(FakeCodex.injects(fake), fn {_, t, _} -> t =~ "what is this?" end)

    assert text =~ "from operator: what is this?\n\n[image attached: #{img.path}]"
    assert %{"type" => "localImage", "path" => img.path} in params["input"]
  end

  test "an agent's reply through the MCP send_message lands in its conversation, live",
       %{a: a, http: http} do
    mcp = NodeClient.start("aperture-mcp.mjs", ["--agent", a, "--hub", http])
    on_exit(fn -> NodeClient.kill(mcp) end)

    mcp_rpc(mcp, 1, "initialize", %{
      protocolVersion: "2025-06-18",
      capabilities: %{},
      clientInfo: %{name: "test", version: "0"}
    })

    NodeClient.write(mcp, ~s({"jsonrpc":"2.0","method":"notifications/initialized"}\n))

    assert %{"result" => %{"content" => [%{"text" => text}]}} =
             mcp_rpc(mcp, 2, "tools/call", %{
               name: "send_message",
               arguments: %{to: "operator", body: "done, see the diff"}
             })

    assert text =~ "operator"

    assert_receive {:chat_message,
                    %{agent: ^a, dir: :in, body: "done, see the diff", read: false, id: id}},
                   2_000

    assert Chat.unread_counts()[a] == 1
    assert [%{id: ^id}] = Chat.history(a)
  end

  test "long messages reach a Claude inbox whole, split into lines the Monitor keeps",
       %{a: a, ws: ws} do
    inbox = NodeClient.start("aperture-inbox.mjs", ["--agent", a, "--url", ws])
    on_exit(fn -> NodeClient.kill(inbox) end)
    wait_connected(a)

    body =
      Enum.map_join(1..60, "\n", fn i ->
        "line #{i}: ação é 😀 " <> String.duplicate("x", rem(i * 37, 90)) <> " \\n literal"
      end)

    {:ok, %{id: id}} = Chat.send(a, body)

    parts = ~r/^\[aperture\] message ##{id} \(part (\d+)\/(\d+)\) from operator: (.*)$/u

    FakeAgent.wait_until(
      fn ->
        case Enum.filter(NodeClient.lines(inbox), &(&1 =~ parts)) do
          [] ->
            false

          ls ->
            length(ls) ==
              ls |> hd() |> then(&Regex.run(parts, &1)) |> Enum.at(2) |> String.to_integer()
        end
      end,
      "not all parts arrived"
    )

    lines = Enum.filter(NodeClient.lines(inbox), &(&1 =~ parts))
    assert length(lines) > 1

    for l <- lines, do: assert(utf16_length(l) <= 480, "line too long: #{utf16_length(l)}")

    numbered = Enum.map(lines, &Regex.run(parts, &1, capture: :all_but_first))
    assert Enum.map(numbered, &String.to_integer(hd(&1))) == Enum.to_list(1..length(lines))

    joined = Enum.map_join(numbered, "", &List.last/1)
    assert joined == String.replace(body, "\n", "\\n")

    # short messages keep the one-line format
    {:ok, %{id: short}} = Chat.send(a, "short one")

    NodeClient.wait_line(
      inbox,
      &(&1 == "[aperture] message ##{short} from operator: short one")
    )
  end

  # -- helpers ----------------------------------------------------------------------

  defp utf16_length(s), do: div(byte_size(:unicode.characters_to_binary(s, :utf8, :utf16)), 2)

  defp wait_connected(agent) do
    FakeAgent.wait_until(
      fn -> agent in Aperture.Router.connected_agents() end,
      "#{agent} never connected"
    )
  end

  defp mcp_rpc(mcp, id, method, params) do
    NodeClient.write(
      mcp,
      JSON.encode!(%{jsonrpc: "2.0", id: id, method: method, params: params}) <> "\n"
    )

    mcp
    |> NodeClient.wait_line(&String.contains?(&1, ~s("id":#{id})))
    |> JSON.decode!()
  end

  defp http_get(port, path, headers \\ []) do
    {:ok, conn} = Mint.HTTP.connect(:http, "127.0.0.1", port, mode: :passive)
    {:ok, conn, ref} = Mint.HTTP.request(conn, "GET", path, headers, nil)
    result = recv(conn, ref, {nil, [], ""})
    Mint.HTTP.close(conn)
    result
  end

  defp recv(conn, ref, {status, headers, body}) do
    {:ok, conn, responses} = Mint.HTTP.recv(conn, 0, 5_000)

    {acc, done} =
      Enum.reduce(responses, {{status, headers, body}, false}, fn
        {:status, ^ref, s}, {{_, h, b}, d} -> {{s, h, b}, d}
        {:headers, ^ref, h}, {{s, hs, b}, d} -> {{s, hs ++ h, b}, d}
        {:data, ^ref, x}, {{s, h, b}, d} -> {{s, h, b <> x}, d}
        {:done, ^ref}, {acc, _} -> {acc, true}
        _, x -> x
      end)

    if done, do: acc, else: recv(conn, ref, acc)
  end
end
