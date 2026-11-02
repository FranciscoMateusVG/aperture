defmodule Aperture.Hub.Socket do
  @moduledoc """
  One process per connected agent. JSON frames, see the v4 spec.

  Client -> hub: `hello`, `ack`, `send`, `ping`
  Hub -> client: `welcome`, `message`, `sent`, `pong`, `error`

  On `hello` the connection registers under the agent name (a newer
  connection replaces an older one) and replays every unacked message.
  When this process dies, the Registry entry disappears with it.
  """
  @behaviour WebSock

  alias Aperture.{Router, Store}

  @impl true
  def init(_), do: {:ok, %{agent: nil}}

  @impl true
  def handle_in({text, [opcode: :text]}, state) do
    case JSON.decode(text) do
      {:ok, frame} -> handle_frame(frame, state)
      {:error, _} -> reply(%{type: "error", reason: "invalid_json"}, state)
    end
  end

  def handle_in(_other, state), do: {:ok, state}

  @impl true
  def handle_info({:push, msg}, state), do: reply(message_frame(msg), state)
  # A newer connection took this agent name. Close with 4001 so the old
  # client knows to exit instead of reconnecting (otherwise two inboxes for
  # the same agent would keep kicking each other off).
  def handle_info(:replaced, state), do: {:stop, :normal, {4001, "replaced"}, state}
  def handle_info(_other, state), do: {:ok, state}

  # -- frames ---------------------------------------------------------------

  defp handle_frame(%{"type" => "hello", "agent" => agent}, %{agent: nil} = state)
       when is_binary(agent) and agent != "" do
    :ok = Router.register(agent)
    replay = Enum.map(Store.unacked(agent), &{:text, JSON.encode!(message_frame(&1))})
    welcome = {:text, JSON.encode!(%{type: "welcome", agent: agent})}
    {:push, [welcome | replay], %{state | agent: agent}}
  end

  defp handle_frame(%{"type" => "hello"}, state),
    do: reply(%{type: "error", reason: "bad_hello"}, state)

  defp handle_frame(_frame, %{agent: nil} = state),
    do: reply(%{type: "error", reason: "hello_first"}, state)

  defp handle_frame(%{"type" => "ack", "id" => id}, state) when is_integer(id) do
    Store.ack(id, state.agent)
    {:ok, state}
  end

  defp handle_frame(%{"type" => "send", "to" => to, "body" => body} = f, state)
       when is_binary(to) and is_binary(body) do
    case Router.send_message(state.agent, to, body) do
      {:ok, id, status} -> reply(%{type: "sent", id: id, status: status, ref: f["ref"]}, state)
      {:error, reason} -> reply(%{type: "error", reason: reason, ref: f["ref"]}, state)
    end
  end

  defp handle_frame(%{"type" => "ping"}, state), do: reply(%{type: "pong"}, state)

  defp handle_frame(_frame, state), do: reply(%{type: "error", reason: "unknown_frame"}, state)

  # -- helpers --------------------------------------------------------------

  defp message_frame(msg),
    do: %{type: "message", id: msg.id, from: msg.from, body: msg.body}

  defp reply(frame, state), do: {:push, {:text, JSON.encode!(frame)}, state}
end
