defmodule Aperture.Store do
  @moduledoc """
  Durable message log (SQLite). The only place messages are stored.

  A message is stored before it is pushed and stays "unacked" until the
  recipient's client acks it. Unacked messages are replayed, in id order,
  every time the recipient connects.
  """
  use GenServer

  alias Exqlite.Sqlite3

  @row_cols "id, from_agent, to_agent, body, inserted_at, acked_at"

  @type attachment :: %{id: String.t(), path: Path.t(), mime: String.t(), size: integer()}
  @type message :: %{
          id: integer(),
          from: String.t(),
          to: String.t(),
          body: String.t(),
          attachments: [attachment()]
        }

  @typedoc "A message with its timestamps, for conversation views (`Aperture.Chat`)."
  @type row :: %{
          id: integer(),
          from: String.t(),
          to: String.t(),
          body: String.t(),
          attachments: [attachment()],
          inserted_at: String.t(),
          acked_at: String.t() | nil
        }

  def start_link(opts), do: GenServer.start_link(__MODULE__, opts, name: __MODULE__)

  @doc "Stores a message (and its attachments, in the same transaction)."
  @spec insert(String.t(), String.t(), String.t(), [attachment()]) :: {:ok, message()}
  def insert(from, to, body, attachments \\ []),
    do: GenServer.call(__MODULE__, {:insert, from, to, body, attachments})

  @doc "Marks a message as acked, only if it belongs to `agent`."
  @spec ack(integer(), String.t()) :: :ok
  def ack(id, agent), do: GenServer.call(__MODULE__, {:ack, id, agent})

  @spec unacked(String.t()) :: [message()]
  def unacked(agent), do: GenServer.call(__MODULE__, {:unacked, agent})

  @spec get(integer()) :: row() | nil
  def get(id), do: GenServer.call(__MODULE__, {:get, id})

  @doc """
  Messages between `a` and `b`, both directions, ascending by id: the last
  `limit` of them, or the last `limit` before id `before`, after `a`'s archive boundary.
  """
  @spec conversation(String.t(), String.t(), pos_integer(), integer() | nil) :: [row()]
  def conversation(a, b, limit, before \\ nil),
    do: GenServer.call(__MODULE__, {:conversation, a, b, limit, before})

  @doc "Archives a viewer/peer conversation through the current log head; never changes messages."
  def archive_conversation(viewer, peer),
    do: GenServer.call(__MODULE__, {:archive_conversation, viewer, peer})

  @doc "The last archived message id for this viewer/peer (zero before the first reset)."
  def archived_through(viewer, peer),
    do: GenServer.call(__MODULE__, {:archived_through, viewer, peer})

  @doc "The latest message `agent` exchanged with each peer, keyed by peer."
  @spec last_per_peer(String.t()) :: %{String.t() => row()}
  def last_per_peer(agent), do: GenServer.call(__MODULE__, {:last_per_peer, agent})

  @doc "How many unacked messages `agent` has from each sender."
  @spec unacked_counts(String.t()) :: %{String.t() => pos_integer()}
  def unacked_counts(agent), do: GenServer.call(__MODULE__, {:unacked_counts, agent})

  @doc """
  Acks what `from` sent to `agent`: everything, `{:up_to, id}` (ids <= id)
  or `{:ids, ids}` (exactly those). Returns how many were acked.
  """
  @spec ack_from(String.t(), String.t(), :all | {:up_to, integer()} | {:ids, [integer()]}) ::
          non_neg_integer()
  def ack_from(agent, from, which \\ :all),
    do: GenServer.call(__MODULE__, {:ack_from, agent, from, which})

  @impl true
  def init(opts) do
    path = Keyword.fetch!(opts, :path)
    File.mkdir_p!(Path.dirname(path))
    {:ok, db} = Sqlite3.open(path)
    :ok = Sqlite3.execute(db, "PRAGMA journal_mode=WAL")

    :ok =
      Sqlite3.execute(db, """
      CREATE TABLE IF NOT EXISTS messages (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        from_agent TEXT NOT NULL,
        to_agent TEXT NOT NULL,
        body TEXT NOT NULL,
        inserted_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
        acked_at TEXT
      )
      """)

    :ok =
      Sqlite3.execute(
        db,
        "CREATE INDEX IF NOT EXISTS messages_unacked ON messages (to_agent, id) WHERE acked_at IS NULL"
      )

    # Additive (chat): conversations are looked up by both ends; images sent
    # with a message are listed here, the files live in the uploads dir.
    :ok =
      Sqlite3.execute(
        db,
        "CREATE INDEX IF NOT EXISTS messages_pair ON messages (from_agent, to_agent, id)"
      )

    :ok =
      Sqlite3.execute(db, """
      CREATE TABLE IF NOT EXISTS message_attachments (
        message_id INTEGER NOT NULL REFERENCES messages(id),
        position INTEGER NOT NULL,
        upload_id TEXT NOT NULL,
        path TEXT NOT NULL,
        mime TEXT NOT NULL,
        size INTEGER NOT NULL,
        PRIMARY KEY (message_id, position)
      )
      """)

    # Append-only reset boundaries: audit survives repeated resets. Message rows,
    # attachment files and delivery acknowledgements remain untouched.
    :ok =
      Sqlite3.execute(db, """
      CREATE TABLE IF NOT EXISTS chat_archives (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        viewer TEXT NOT NULL,
        peer TEXT NOT NULL,
        through_id INTEGER NOT NULL,
        inserted_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
      )
      """)

    :ok =
      Sqlite3.execute(
        db,
        "CREATE INDEX IF NOT EXISTS chat_archives_pair ON chat_archives (viewer, peer, through_id)"
      )

    {:ok, %{db: db}}
  end

  @impl true
  def handle_call({:insert, from, to, body, attachments}, _from, %{db: db} = state) do
    :ok = Sqlite3.execute(db, "BEGIN")

    try do
      run(db, "INSERT INTO messages (from_agent, to_agent, body) VALUES (?1, ?2, ?3)", [
        from,
        to,
        body
      ])

      {:ok, id} = Sqlite3.last_insert_rowid(db)

      for {a, pos} <- Enum.with_index(attachments) do
        run(
          db,
          "INSERT INTO message_attachments (message_id, position, upload_id, path, mime, size) " <>
            "VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
          [id, pos, a.id, a.path, a.mime, a.size]
        )
      end

      :ok = Sqlite3.execute(db, "COMMIT")

      {:reply, {:ok, %{id: id, from: from, to: to, body: body, attachments: attachments}}, state}
    rescue
      e ->
        Sqlite3.execute(db, "ROLLBACK")
        reraise e, __STACKTRACE__
    end
  end

  def handle_call({:ack, id, agent}, _from, %{db: db} = state) do
    run(
      db,
      "UPDATE messages SET acked_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') " <>
        "WHERE id = ?1 AND to_agent = ?2 AND acked_at IS NULL",
      [id, agent]
    )

    {:reply, :ok, state}
  end

  def handle_call({:unacked, agent}, _from, %{db: db} = state) do
    rows =
      run(
        db,
        "SELECT id, from_agent, to_agent, body FROM messages " <>
          "WHERE to_agent = ?1 AND acked_at IS NULL ORDER BY id",
        [agent]
      )

    msgs =
      Enum.map(rows, fn [id, from, to, body] -> %{id: id, from: from, to: to, body: body} end)

    {:reply, with_attachments(db, msgs), state}
  end

  def handle_call({:get, id}, _from, %{db: db} = state) do
    rows = run(db, "SELECT #{@row_cols} FROM messages WHERE id = ?1", [id])
    {:reply, db |> to_rows(rows) |> List.first(), state}
  end

  def handle_call({:archive_conversation, viewer, peer}, _from, %{db: db} = state) do
    # One SQLite statement, serialized with message inserts: concurrent messages
    # are unambiguously before or after this boundary.
    run(
      db,
      "INSERT INTO chat_archives (viewer, peer, through_id) " <>
        "SELECT ?1, ?2, coalesce(max(id), 0) FROM messages",
      [viewer, peer]
    )

    {:reply, :ok, state}
  end

  def handle_call({:archived_through, viewer, peer}, _from, %{db: db} = state) do
    [[id]] =
      run(
        db,
        "SELECT coalesce(max(through_id), 0) FROM chat_archives WHERE viewer = ?1 AND peer = ?2",
        [viewer, peer]
      )

    {:reply, id, state}
  end

  def handle_call({:conversation, a, b, limit, before}, _from, %{db: db} = state) do
    rows =
      run(
        db,
        "SELECT #{@row_cols} FROM messages " <>
          "WHERE ((from_agent = ?1 AND to_agent = ?2) OR (from_agent = ?2 AND to_agent = ?1)) " <>
          "AND id > (SELECT coalesce(max(through_id), 0) FROM chat_archives WHERE viewer = ?1 AND peer = ?2) " <>
          "AND id < ?3 ORDER BY id DESC LIMIT ?4",
        [a, b, before || 9_223_372_036_854_775_807, limit]
      )

    {:reply, db |> to_rows(Enum.reverse(rows)), state}
  end

  def handle_call({:last_per_peer, agent}, _from, %{db: db} = state) do
    rows =
      run(
        db,
        "SELECT #{@row_cols} FROM messages WHERE id IN (" <>
          "SELECT max(id) FROM messages WHERE (from_agent = ?1 OR to_agent = ?1) " <>
          "AND id > (SELECT coalesce(max(through_id), 0) FROM chat_archives WHERE viewer = ?1 " <>
          "AND peer = CASE WHEN from_agent = ?1 THEN to_agent ELSE from_agent END) " <>
          "GROUP BY CASE WHEN from_agent = ?1 THEN to_agent ELSE from_agent END)",
        [agent]
      )

    {:reply,
     db
     |> to_rows(rows)
     |> Map.new(&{if(&1.from == agent, do: &1.to, else: &1.from), &1}), state}
  end

  def handle_call({:unacked_counts, agent}, _from, %{db: db} = state) do
    rows =
      run(
        db,
        "SELECT from_agent, count(*) FROM messages " <>
          "WHERE to_agent = ?1 AND acked_at IS NULL " <>
          "AND id > (SELECT coalesce(max(through_id), 0) FROM chat_archives WHERE viewer = ?1 AND peer = from_agent) " <>
          "GROUP BY from_agent",
        [agent]
      )

    {:reply, Map.new(rows, fn [from, n] -> {from, n} end), state}
  end

  def handle_call({:ack_from, _agent, _sender, {:ids, []}}, _from, state),
    do: {:reply, 0, state}

  def handle_call({:ack_from, agent, from, which}, _from, %{db: db} = state) do
    {cond, args} =
      case which do
        :all ->
          {"", []}

        {:up_to, id} ->
          {" AND id <= ?3", [id]}

        {:ids, ids} ->
          {" AND id IN (#{Enum.map_join(3..(length(ids) + 2), ",", &"?#{&1}")})", ids}
      end

    run(
      db,
      "UPDATE messages SET acked_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') " <>
        "WHERE to_agent = ?1 AND from_agent = ?2 AND acked_at IS NULL" <> cond,
      [agent, from | args]
    )

    {:ok, n} = Sqlite3.changes(db)
    {:reply, n, state}
  end

  @impl true
  def terminate(_reason, %{db: db}), do: Sqlite3.close(db)

  defp to_rows(db, rows) do
    msgs =
      for [id, from, to, body, inserted_at, acked_at] <- rows do
        %{
          id: id,
          from: from,
          to: to,
          body: body,
          inserted_at: inserted_at,
          acked_at: acked_at
        }
      end

    with_attachments(db, msgs)
  end

  defp with_attachments(_db, []), do: []

  defp with_attachments(db, msgs) do
    ids = Enum.map(msgs, & &1.id)
    placeholders = Enum.map_join(1..length(ids), ",", &"?#{&1}")

    by_msg =
      run(
        db,
        "SELECT message_id, upload_id, path, mime, size FROM message_attachments " <>
          "WHERE message_id IN (#{placeholders}) ORDER BY message_id, position",
        ids
      )
      |> Enum.group_by(&hd/1, fn [_, id, path, mime, size] ->
        %{id: id, path: path, mime: mime, size: size}
      end)

    Enum.map(msgs, &Map.put(&1, :attachments, Map.get(by_msg, &1.id, [])))
  end

  defp run(db, sql, args) do
    {:ok, stmt} = Sqlite3.prepare(db, sql)

    try do
      :ok = Sqlite3.bind(stmt, args)
      {:ok, rows} = Sqlite3.fetch_all(db, stmt)
      rows
    after
      Sqlite3.release(db, stmt)
    end
  end
end
