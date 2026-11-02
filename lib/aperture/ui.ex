defmodule Aperture.UI do
  @moduledoc """
  Data behind the launcher page (`GET /`): every agent with harness, model and
  live state, how many unread operator messages each has sent, and the
  operator inbox itself. Agents reach the operator with
  `send_message(to: "operator")`; those messages wait here until acked.

  `stream/1` serves it as Server-Sent Events: a full snapshot whenever
  anything changes (checked every second), plus a keep-alive comment.
  """
  import Plug.Conn

  @operator "operator"
  @tick_ms 1_000
  @keepalive_ticks 15

  @spec snapshot() :: map()
  def snapshot do
    inbox =
      for m <- Aperture.Store.unacked(@operator),
          do: %{id: m.id, from: m.from, body: m.body}

    unread = Enum.frequencies_by(inbox, & &1.from)
    current = Aperture.Tmux.current_window(Application.fetch_env!(:aperture, :tmux_session))

    agents =
      for a <- Aperture.Agents.list() do
        %{
          name: a.name,
          display: a.display,
          role: a.role,
          harness: a.harness,
          model: a.model,
          state: Aperture.Presence.state(a.name),
          unread: Map.get(unread, a.name, 0),
          focused: current == a.name,
          team: a.team
        }
      end

    %{
      agents: agents,
      inbox: inbox,
      harnesses: Aperture.Agents.harnesses(),
      models: Aperture.Agents.models(),
      teams:
        for t <- Aperture.Teams.list() do
          %{
            name: t["name"],
            lead: t["lead"],
            project: t["project"],
            base: t["base"],
            members: Enum.map(t["members"], & &1["name"]),
            roster:
              for m <- t["members"] do
                %{name: m["name"], role: m["role"], branch: m["branch"]}
              end
          }
        end
    }
  end

  @spec stream(Plug.Conn.t()) :: Plug.Conn.t()
  def stream(conn) do
    conn =
      conn
      |> put_resp_header("content-type", "text/event-stream")
      |> put_resp_header("cache-control", "no-cache")
      |> send_chunked(200)

    loop(conn, nil, 0)
  end

  defp loop(conn, last, ticks) do
    snap = snapshot()

    {result, last} =
      cond do
        snap != last ->
          {chunk(conn, "data: " <> JSON.encode!(snap) <> "\n\n"), snap}

        rem(ticks, @keepalive_ticks) == @keepalive_ticks - 1 ->
          {chunk(conn, ": keep-alive\n\n"), last}

        true ->
          {{:ok, conn}, last}
      end

    case result do
      {:ok, conn} ->
        Process.sleep(@tick_ms)
        loop(conn, last, ticks + 1)

      {:error, _closed} ->
        conn
    end
  end
end
