defmodule Aperture.Hub.Endpoint do
  @moduledoc """
  HTTP entry point (bound to 127.0.0.1; local requests only, see `Aperture.Hub.LocalOnly`).

    * `GET /ui/state`, `GET /ui/events` (SSE): the launcher's data as JSON (the
      page itself is `Aperture.Web.DashboardLive`, see `Aperture.Web.Front`)
    * `POST /agents/:name/focus` switches the tmux session to that agent's window
    * `POST /operator/ack` `{"id"}` marks an operator message read
    * `GET  /inbox`    upgrades to the agent WebSocket (receive side)
    * `POST /messages` sends a message: `{"from", "to", "body"}` (used by the MCP shim)
    * `GET  /agents`   names of currently connected agents
    * `GET  /presence` every defined agent: harness, model, offline/idle/busy
    * `POST /presence/:agent` `{"state": "busy"|"idle"}` (Claude Code hooks)
    * `POST /agents/:name/config` `{"harness"?, "model"?}` saves and restarts that agent
    * `POST /agents/:name/restart` | `/stop` | `/start` (async, 202)
    * `GET  /teams`; `POST /teams` `{"from", "name", "project", "base"?, "members"}` creates
      a team (leads only) and returns once its members are launched;
      `POST /teams/:name/dissolve` `{"from"}` (its lead, or `operator` from the page)
    * `POST /beads/:op` `{"from", ...}` BEADS task ops (create, list, show, claim, update, close)
    * `POST /recall` `{"query", "k"?}` and `POST /recall/full` `{"key"}`: v3's recall memory, read-only
    * `POST /codex/bridges` `{"agent", "socket"}` starts a Codex bridge
    * `DELETE /codex/bridges/:agent` stops it
    * `GET  /uploads/:id` an image attached in the chat (`Aperture.Chat.Uploads`)
    * `GET  /health`
  """
  use Plug.Router

  plug(Aperture.Hub.LocalOnly)
  plug(:match)
  plug(Plug.Parsers, parsers: [:json], json_decoder: JSON, pass: ["*/*"])
  plug(:dispatch)

  # -- launcher page ----------------------------------------------------------

  get "/ui/state" do
    json(conn, 200, Aperture.UI.snapshot())
  end

  get "/ui/events" do
    Aperture.UI.stream(conn)
  end

  post "/agents/:name/focus" do
    session = Application.fetch_env!(:aperture, :tmux_session)

    with %{} <- Aperture.Agents.get(name),
         :ok <- Aperture.Tmux.select_window(session, name) do
      json(conn, 200, %{agent: name, focused: true})
    else
      _ -> json(conn, 404, %{error: "#{name} has no window"})
    end
  end

  post "/operator/ack" do
    case conn.body_params do
      %{"id" => id} when is_integer(id) ->
        Aperture.Store.ack(id, "operator")
        json(conn, 200, %{acked: id})

      _ ->
        json(conn, 400, %{error: "expected id"})
    end
  end

  # -- agents' socket -----------------------------------------------------------

  get "/inbox" do
    conn
    |> WebSockAdapter.upgrade(Aperture.Hub.Socket, [], timeout: 60_000)
    |> halt()
  end

  post "/messages" do
    case conn.body_params do
      %{"from" => from, "to" => to, "body" => body}
      when is_binary(from) and from != "" and is_binary(to) and to != "" and is_binary(body) ->
        case Aperture.Router.send_message(from, to, body) do
          {:ok, id, status} -> json(conn, 200, %{id: id, status: status})
          {:error, reason} -> json(conn, 403, %{error: reason})
        end

      _ ->
        json(conn, 400, %{error: "expected from, to, body"})
    end
  end

  get "/agents" do
    json(conn, 200, %{agents: Enum.sort(Aperture.Router.connected_agents())})
  end

  get "/presence" do
    agents =
      for a <- Aperture.Agents.list() do
        %{
          name: a.name,
          display: a.display,
          role: a.role,
          harness: a.harness,
          model: a.model,
          state: Aperture.Presence.state(a.name)
        }
      end

    json(conn, 200, %{agents: agents})
  end

  post "/presence/:agent" do
    case conn.body_params do
      %{"state" => state} when state in ["busy", "idle"] ->
        if Aperture.Router.valid_name?(agent),
          do: Aperture.Presence.set(agent, String.to_existing_atom(state))

        json(conn, 200, %{agent: agent, state: state})

      _ ->
        json(conn, 400, %{error: "expected state: busy | idle"})
    end
  end

  post "/agents/:name/config" do
    opts =
      [harness: conn.body_params["harness"], model: conn.body_params["model"]]
      |> Enum.reject(fn {_key, value} -> is_nil(value) end)

    case Aperture.Agents.set(name, opts) do
      {:ok, agent} ->
        Task.start(fn -> Aperture.Launcher.restart_agent(name) end)

        json(conn, 202, %{
          agent: name,
          harness: agent.harness,
          model: agent.model,
          status: "restarting"
        })

      {:error, reason} ->
        json(conn, 400, %{error: to_string(reason)})
    end
  end

  post "/agents/:name/restart" do
    lifecycle(conn, name, &Aperture.Launcher.restart_agent/1, "restarting")
  end

  post "/agents/:name/stop" do
    lifecycle(conn, name, &Aperture.Launcher.stop_agent/1, "stopping")
  end

  post "/agents/:name/start" do
    lifecycle(conn, name, &Aperture.Launcher.start_agent/1, "starting")
  end

  get "/teams" do
    json(conn, 200, %{teams: Aperture.Teams.list()})
  end

  post "/teams" do
    conn.body_params["from"]
    |> Aperture.Teams.create(Map.delete(conn.body_params, "from"))
    |> team_reply(conn)
  end

  post "/teams/:name/dissolve" do
    conn.body_params["from"]
    |> Aperture.Teams.dissolve(name)
    |> team_reply(conn)
  end

  post "/beads/:op" do
    case Aperture.Beads.run(op, conn.body_params["from"], conn.body_params) do
      {:ok, result} -> json(conn, 200, %{result: result})
      {:indeterminate, result} -> json(conn, 409, %{error: result["message"], result: result})
      {:error, reason} -> json(conn, 422, %{error: reason})
    end
  end

  post "/recall" do
    k = if is_integer(conn.body_params["k"]), do: conn.body_params["k"], else: 5

    case Aperture.Recall.search(conn.body_params["query"], k) do
      {:ok, items} -> json(conn, 200, %{result: items})
      {:error, reason} -> json(conn, 422, %{error: reason})
    end
  end

  post "/recall/full" do
    case Aperture.Recall.full(conn.body_params["key"]) do
      {:ok, memory} -> json(conn, 200, %{result: memory})
      {:error, reason} -> json(conn, 422, %{error: reason})
    end
  end

  post "/codex/bridges" do
    case conn.body_params do
      %{"agent" => agent, "socket" => socket} ->
        case Aperture.Codex.start_bridge(agent, socket) do
          {:ok, _pid} -> json(conn, 200, %{agent: agent, status: "started"})
          {:error, reason} -> json(conn, 400, %{error: inspect(reason)})
        end

      _ ->
        json(conn, 400, %{error: "expected agent, socket"})
    end
  end

  delete "/codex/bridges/:agent" do
    case Aperture.Codex.stop_bridge(agent) do
      :ok -> json(conn, 200, %{agent: agent, status: "stopped"})
      {:error, :not_found} -> json(conn, 404, %{error: "no bridge for #{agent}"})
    end
  end

  get "/uploads/:id" do
    case Aperture.Chat.Uploads.fetch(id) do
      {:ok, upload} ->
        conn
        |> put_resp_content_type(upload.mime, nil)
        |> put_resp_header("x-content-type-options", "nosniff")
        |> put_resp_header("content-security-policy", "default-src 'none'; sandbox")
        |> put_resp_header("cache-control", "private, max-age=31536000, immutable")
        |> send_file(200, upload.path)

      :error ->
        send_resp(conn, 404, "not found")
    end
  end

  get "/health" do
    beads = Aperture.Beads.Queue.health()
    conn = put_resp_header(conn, "x-aperture-beads", beads)

    # Liveness is used by the Dock/launchd smoke checks to decide whether to
    # start a hub. A blocked task queue must never cause a second hub launch.
    send_resp(conn, 200, "ok; bd queue " <> beads)
  end

  match _ do
    send_resp(conn, 404, "not found")
  end

  # launches and kills take seconds (tmux, Codex app-server), so the page
  # gets 202 right away and watches presence for the result
  defp lifecycle(conn, name, fun, status) do
    if Aperture.Agents.get(name) do
      Task.start(fn -> fun.(name) end)
      json(conn, 202, %{agent: name, status: status})
    else
      json(conn, 404, %{error: "unknown agent"})
    end
  end

  defp team_reply({:ok, result}, conn), do: json(conn, 200, %{result: result})

  defp team_reply({:error, kind, msg}, conn) do
    status = %{forbidden: 403, not_found: 404}[kind] || 422
    json(conn, status, %{error: msg})
  end

  defp json(conn, status, data) do
    conn
    |> put_resp_content_type("application/json")
    |> send_resp(status, JSON.encode!(data))
  end
end
