defmodule Aperture.Chat do
  @moduledoc """
  The operator's conversations with each agent.

  A conversation is just the messages between `operator` and that agent in
  the message log (`Aperture.Store`): `:out` is operator -> agent, `:in` is
  agent -> operator. An `:in` message is unread until acked (`mark_read/1`),
  the same state the old operator inbox used; an `:out` message is delivered
  once the agent's client acked it.

  The operator's messages go through `Aperture.Router` like any other, so
  they reach Claude agents through their inbox socket and Codex agents as an
  injected turn. Attached images are stored by `Aperture.Chat.Uploads`; the
  agent gets each one's absolute path appended to the text (and Codex also
  as an image input).

  Live updates go out on `Aperture.PubSub`, topic `"chat"`:
    * `{:chat_message, message}` for every new message, either direction
    * `{:chat_read, agent}` when that agent's unread count went down
    * `{:chat_reset, agent}` when a conversation was archived
  """

  alias Aperture.{Router, Store}
  alias Aperture.Chat.Uploads

  @operator "operator"
  @topic "chat"

  @type attachment :: Uploads.t()
  @type message :: %{
          id: integer(),
          agent: String.t(),
          dir: :in | :out,
          body: String.t(),
          attachments: [attachment()],
          at: String.t(),
          read: boolean(),
          delivered: boolean()
        }

  @spec topic() :: String.t()
  def topic, do: @topic

  @spec subscribe() :: :ok | {:error, term()}
  def subscribe, do: Phoenix.PubSub.subscribe(Aperture.PubSub, @topic)

  @doc "Messages with `agent`, ascending by id: the last `limit`, or the last `limit` before id `before`."
  @spec history(String.t(), pos_integer(), keyword()) :: [message()]
  def history(agent, limit \\ 200, opts \\ []) do
    @operator
    |> Store.conversation(agent, limit, opts[:before])
    |> Enum.map(&to_message/1)
  end

  @doc """
  Archives this conversation in the hub, without deleting or acknowledging any
  message. History, previews and badges begin after the durable reset boundary.
  This does not cancel pending deliveries or stop an agent's work.
  """
  @spec reset(String.t()) :: :ok | {:error, String.t()}
  def reset(agent) do
    with :ok <- check_agent(agent),
         :ok <- Store.archive_conversation(@operator, agent) do
      broadcast({:chat_reset, agent})
    end
  end

  @doc "Rejects delayed PubSub messages from before a reset."
  def archived?(agent, id), do: id <= Store.archived_through(@operator, agent)

  @doc "Every defined agent with its unread count and last message, latest activity first."
  @spec conversations() :: [
          %{agent: String.t(), unread: non_neg_integer(), last: message() | nil}
        ]
  def conversations do
    unread = unread_counts()
    last = Store.last_per_peer(@operator)

    Aperture.Agents.list()
    |> Enum.map(fn a ->
      %{
        agent: a.name,
        unread: Map.get(unread, a.name, 0),
        last: last |> Map.get(a.name) |> then(&(&1 && to_message(&1)))
      }
    end)
    |> Enum.sort_by(&if(&1.last, do: &1.last.id, else: 0), :desc)
  end

  @doc "Unread `:in` messages per agent (agents with none are left out)."
  @spec unread_counts() :: %{String.t() => pos_integer()}
  def unread_counts, do: Store.unacked_counts(@operator)

  @spec unread_total() :: non_neg_integer()
  def unread_total, do: unread_counts() |> Map.values() |> Enum.sum()

  @doc """
  Marks what `agent` sent the operator as read: everything, or only
  `ids: [id]` (exactly the messages the operator has seen; other agents' or
  outgoing ids are ignored), or `up_to: id` (every one with id <= id).
  Broadcasts `{:chat_read, agent}` when anything changed; the agent may
  still have unread messages after a partial read.
  """
  @spec mark_read(String.t(), keyword()) :: :ok
  def mark_read(agent, opts \\ []) do
    which =
      cond do
        ids = opts[:ids] -> {:ids, Enum.filter(ids, &is_integer/1)}
        id = opts[:up_to] -> {:up_to, id}
        true -> :all
      end

    if Store.ack_from(@operator, agent, which) > 0, do: broadcast({:chat_read, agent})
    :ok
  end

  @doc """
  Send `body` from the operator to `agent`, with the uploads `upload_ids`
  (ids returned by `store_upload/1`) attached.
  """
  @spec send(String.t(), String.t(), [String.t()]) :: {:ok, message()} | {:error, String.t()}
  def send(agent, body, upload_ids \\ []) do
    with :ok <- check_agent(agent),
         {:ok, attachments} <- fetch_uploads(upload_ids),
         :ok <- check_body(body, attachments),
         {:ok, id, _status} <-
           Router.send_message(@operator, agent, delivery_text(body, attachments), attachments) do
      {:ok, id |> Store.get() |> to_message()}
    end
  end

  @spec store_upload(Path.t()) :: {:ok, attachment()} | {:error, Uploads.error()}
  defdelegate store_upload(tmp_path), to: Uploads, as: :store_file

  @spec store_upload_binary(binary()) :: {:ok, attachment()} | {:error, Uploads.error()}
  defdelegate store_upload_binary(data), to: Uploads, as: :store_binary

  @doc "Called by `Aperture.Router` for every stored message; publishes the operator's."
  @spec notify(Store.message()) :: :ok
  def notify(%{from: from, to: to, id: id}) when @operator in [from, to] do
    case Store.get(id) do
      nil -> :ok
      row -> broadcast({:chat_message, to_message(row)})
    end
  end

  def notify(_msg), do: :ok

  @doc "The text an agent receives: the body, then one line per image with its absolute path."
  @spec delivery_text(String.t(), [attachment()]) :: String.t()
  def delivery_text(body, []), do: body

  def delivery_text(body, attachments) do
    lines = Enum.map_join(attachments, "\n", &"[image attached: #{&1.path}]")
    if body == "", do: lines, else: body <> "\n\n" <> lines
  end

  # -- helpers ------------------------------------------------------------------

  defp check_agent(@operator), do: {:error, "cannot message the operator from the chat"}

  defp check_agent(agent) do
    if Router.valid_name?(agent), do: :ok, else: {:error, "invalid agent name"}
  end

  defp check_body(body, attachments) when is_binary(body) do
    if String.trim(body) == "" and attachments == [],
      do: {:error, "empty message"},
      else: :ok
  end

  defp check_body(_, _), do: {:error, "body must be a string"}

  defp fetch_uploads(ids) when is_list(ids) do
    Enum.reduce_while(ids, {:ok, []}, fn id, {:ok, acc} ->
      case Uploads.fetch(id) do
        {:ok, upload} -> {:cont, {:ok, [upload | acc]}}
        :error -> {:halt, {:error, "unknown upload #{inspect(id)}"}}
      end
    end)
    |> case do
      {:ok, list} -> {:ok, Enum.reverse(list)}
      error -> error
    end
  end

  defp fetch_uploads(_), do: {:error, "upload ids must be a list"}

  defp to_message(%{from: @operator} = row) do
    attachments = Enum.map(row.attachments, &Map.put(&1, :url, "/uploads/" <> &1.id))

    %{
      id: row.id,
      agent: row.to,
      dir: :out,
      body: strip_suffix(row.body, attachments),
      attachments: attachments,
      at: row.inserted_at,
      read: true,
      delivered: row.acked_at != nil
    }
  end

  defp to_message(row) do
    %{
      id: row.id,
      agent: row.from,
      dir: :in,
      body: row.body,
      attachments: [],
      at: row.inserted_at,
      read: row.acked_at != nil,
      delivered: true
    }
  end

  # the stored body is what the agent got; the chat shows what was typed
  defp strip_suffix(body, []), do: body

  defp strip_suffix(body, attachments) do
    full = delivery_text("", attachments)

    cond do
      body == full -> ""
      String.ends_with?(body, "\n\n" <> full) -> String.replace_suffix(body, "\n\n" <> full, "")
      true -> body
    end
  end

  defp broadcast(event) do
    Phoenix.PubSub.broadcast(Aperture.PubSub, @topic, event)
    :ok
  end
end
