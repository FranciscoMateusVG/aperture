defmodule Aperture.Web.DashboardLive do
  @moduledoc """
  The launcher page (`GET /`): every agent with harness, model and live state,
  the teams, and the operator's chat with each agent.

  Live state is `Aperture.UI.snapshot/0`, re-read every second and assigned
  only when it changed (like the old SSE feed). Edits in progress (a harness
  or model picked but not applied) live in `drafts`, so a refresh never
  undoes them.

  Destructive buttons (Stop, Dissolve) need a second click within 3s.

  The Chat modal is the operator's conversation with each agent
  (`Aperture.Chat`): history, unread badges, and images pasted (Cmd+V),
  dropped or picked, previewed before sending. New messages arrive over
  PubSub (`Aperture.Chat.subscribe/0`).
  """
  use Phoenix.LiveView

  alias Aperture.{Agents, Chat, Launcher, Teams, Tmux}
  alias Aperture.Web.TeamTasks

  @tick_ms 1_000
  @arm_ms 3_000
  @toast_ms 2_600
  @other "__other__"
  @max_image_bytes 10 * 1024 * 1024
  @page 200

  # -- lifecycle ----------------------------------------------------------------

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      :timer.send_interval(@tick_ms, :tick)
      Chat.subscribe()
    end

    {:ok,
     socket
     |> assign(drafts: %{}, armed: MapSet.new(), dissolving: MapSet.new(), toast: nil)
     |> assign(chat_open: false, chat_agent: nil, chat_reset: nil, unread: Chat.unread_counts())
     # the open conversation's oldest loaded id, its loaded unread ids, and
     # whether the operator is looking at the page (reported by the browser)
     # not visible until the connected browser says so: a plain render (or a
     # page restored in the background) must not mark anything read
     |> assign(chat_oldest: nil, chat_more: false, chat_unseen: [], visible: false)
     |> assign(expanded: MapSet.new(), tasks: TeamTasks.current())
     |> stream(:messages, [])
     |> allow_upload(:images,
       accept: ~w(.png .jpg .jpeg .webp .gif),
       max_entries: 6,
       max_file_size: @max_image_bytes
     )
     |> load()
     |> load_unread()}
  end

  defp load(socket) do
    snap = Aperture.UI.snapshot()

    if socket.assigns[:snap] == snap, do: socket, else: assign(socket, snap: snap)
  end

  defp load_unread(socket) do
    unread = Chat.unread_counts()
    total = unread |> Map.values() |> Enum.sum()

    socket
    |> assign(
      unread: unread,
      page_title: if(total > 0, do: "(#{total}) Aperture", else: "Aperture")
    )
    |> push_event("unread", %{total: total})
  end

  # /?chat=<agent> (a notification clicked while no window was open)
  @impl true
  def handle_params(%{"chat" => agent}, _uri, socket) when is_binary(agent) do
    if agent(socket, agent),
      do: handle_event("chat_open", %{"agent" => agent}, socket),
      else: {:noreply, socket}
  end

  def handle_params(_params, _uri, socket), do: {:noreply, socket}

  @impl true
  def handle_info(:tick, socket) do
    tasks = TeamTasks.current()
    socket = if tasks == socket.assigns.tasks, do: socket, else: assign(socket, tasks: tasks)
    {:noreply, load(socket)}
  end

  def handle_info({:disarm, key}, socket),
    do: {:noreply, update(socket, :armed, &MapSet.delete(&1, key))}

  def handle_info({:chat_reset, name}, socket) do
    socket =
      if socket.assigns.chat_open and socket.assigns.chat_agent == name,
        do: select_chat(socket, name),
        else: load_unread(socket)

    {:noreply, socket}
  end

  def handle_info({:chat_message, m}, socket) do
    if Chat.archived?(m.agent, m.id),
      do: {:noreply, load_unread(socket)},
      else: show_chat_message(m, socket)
  end

  def handle_info({:chat_read, _agent}, socket), do: {:noreply, load_unread(socket)}

  def handle_info({:clear_toast, ref}, socket) do
    case socket.assigns.toast do
      %{ref: ^ref} -> {:noreply, assign(socket, toast: nil)}
      _ -> {:noreply, socket}
    end
  end

  # -- agent controls -------------------------------------------------------------

  @impl true
  def handle_event("focus", %{"agent" => name}, socket) do
    session = Application.fetch_env!(:aperture, :tmux_session)

    with %{} <- Agents.get(name), :ok <- Tmux.select_window(session, name) do
      {:noreply, socket |> toast("tmux → #{name}") |> load()}
    else
      _ -> {:noreply, toast(socket, "#{name} has no window", :error)}
    end
  end

  def handle_event("config_change", %{"agent" => name} = params, socket) do
    case agent(socket, name) do
      nil ->
        {:noreply, socket}

      a ->
        old = draft(socket, a)
        harness = params["harness"] || old.harness

        draft =
          cond do
            harness != old.harness ->
              %{harness: harness, model: first_model(socket, a, harness), custom: false}

            params["model"] == @other ->
              %{old | custom: true, model: ""}

            old.custom ->
              %{old | model: String.trim(params["custom"] || "")}

            true ->
              %{old | model: params["model"] || old.model}
          end

        {:noreply, put_draft(socket, a, draft)}
    end
  end

  def handle_event("custom_escape", %{"agent" => name}, socket) do
    case agent(socket, name) do
      nil ->
        {:noreply, socket}

      a ->
        {:noreply,
         put_draft(socket, a, %{draft(socket, a) | custom: false, model: a.model || ""})}
    end
  end

  def handle_event("apply", %{"agent" => name}, socket) do
    with %{} = a <- agent(socket, name),
         d = draft(socket, a),
         true <- dirty?(a, d),
         {:ok, saved} <- Agents.set(name, harness: d.harness, model: d.model) do
      Task.start(fn -> Launcher.restart_agent(name) end)

      {:noreply,
       socket
       |> update(:drafts, &Map.delete(&1, name))
       |> toast("Restarting #{a.display} on #{saved.harness} · #{saved.model}")}
    else
      {:error, reason} -> {:noreply, toast(socket, to_string(reason), :error)}
      _ -> {:noreply, socket}
    end
  end

  def handle_event("restart", %{"agent" => name}, socket),
    do: lifecycle(socket, name, &Launcher.restart_agent/1, "Restarting")

  def handle_event("power", %{"agent" => name}, socket) do
    key = {:stop, name}

    case agent(socket, name) do
      nil ->
        {:noreply, socket}

      %{state: state} when state in [:offline, :startup_failed] ->
        lifecycle(socket, name, &Launcher.start_agent/1, "Starting")

      _ ->
        if MapSet.member?(socket.assigns.armed, key) do
          socket = update(socket, :armed, &MapSet.delete(&1, key))
          lifecycle(socket, name, &Launcher.stop_agent/1, "Stopping")
        else
          {:noreply, arm(socket, key)}
        end
    end
  end

  # -- chat -----------------------------------------------------------------------

  def handle_event("chat_open", params, socket) do
    agent =
      params["agent"] || socket.assigns.chat_agent ||
        Enum.find_value(
          socket.assigns.snap.agents,
          &(Map.has_key?(socket.assigns.unread, &1.name) && &1.name)
        ) ||
        socket.assigns.snap.agents |> List.first() |> then(&(&1 && &1.name))

    {:noreply, socket |> assign(chat_open: true) |> select_chat(agent)}
  end

  def handle_event("chat_close", _params, socket),
    do: {:noreply, socket |> cancel_uploads() |> assign(chat_open: false, chat_reset: nil)}

  def handle_event("chat_select", %{"agent" => agent}, socket),
    do: {:noreply, select_chat(socket, agent)}

  def handle_event("chat_reset", _params, socket) do
    name = socket.assigns.chat_agent

    if socket.assigns.chat_open and agent(socket, name),
      do: {:noreply, assign(socket, chat_reset: name)},
      else: {:noreply, socket}
  end

  def handle_event("chat_reset_cancel", _params, socket),
    do: {:noreply, assign(socket, chat_reset: nil)}

  def handle_event("chat_reset_confirm", %{"agent" => name} = params, socket) do
    if socket.assigns.chat_open and socket.assigns.chat_reset == name and
         socket.assigns.chat_agent == name and agent(socket, name) do
      case Chat.reset(name) do
        :ok ->
          socket =
            socket |> select_chat(name) |> toast("Conversation archived; no messages deleted")

          socket =
            if params["restart"] == "true",
              do:
                start_async(socket, {:chat_restart, name}, fn ->
                  Launcher.restart_agent(name, fresh_context: true)
                end),
              else: socket

          {:noreply, socket}

        {:error, reason} ->
          {:noreply, toast(socket, reason, :error)}
      end
    else
      {:noreply, toast(socket, "Reset cancelled: conversation changed or not confirmed", :error)}
    end
  end

  def handle_event("chat_older", _params, socket) do
    %{chat_agent: agent, chat_oldest: oldest} = socket.assigns

    if agent && oldest do
      older = Chat.history(agent, @page, before: oldest)

      socket =
        older
        |> Enum.reverse()
        |> Enum.reduce(socket, &stream_insert(&2, :messages, &1, at: 0))
        |> assign(
          chat_oldest: if(older == [], do: oldest, else: hd(older).id),
          chat_more: length(older) == @page
        )
        |> add_unseen(older)
        |> mark_shown_read()

      {:noreply, load_unread(socket)}
    else
      {:noreply, socket}
    end
  end

  # the browser reports whether the page is visible and focused: only then
  # does an open conversation count as read
  def handle_event("visibility", %{"visible" => visible}, socket) do
    socket = assign(socket, visible: visible == true)
    {:noreply, socket |> mark_shown_read() |> load_unread()}
  end

  def handle_event("chat_validate", _params, socket), do: {:noreply, socket}

  def handle_event("chat_cancel_upload", %{"ref" => ref}, socket),
    do: {:noreply, cancel_upload(socket, :images, ref)}

  # the form names its recipient: a message composed for one conversation is
  # never delivered to another the operator switched to meanwhile
  def handle_event("chat_send", %{"agent" => agent}, socket)
      when agent != socket.assigns.chat_agent do
    {:noreply, toast(socket, "conversation changed; message not sent", :error)}
  end

  def handle_event("chat_send", params, socket) do
    agent = socket.assigns.chat_agent
    body = String.trim_trailing(params["body"] || "")

    stored =
      consume_uploaded_entries(socket, :images, fn %{path: path}, _entry ->
        {:ok, Chat.store_upload(path)}
      end)

    {ids, errors} =
      Enum.reduce(stored, {[], []}, fn
        {:ok, att}, {ids, errs} -> {[att.id | ids], errs}
        {:error, reason}, {ids, errs} -> {ids, [reason | errs]}
      end)

    cond do
      errors != [] ->
        {:noreply,
         toast(socket, "image refused: #{Enum.map_join(errors, ", ", &upload_error/1)}", :error)}

      String.trim(body) == "" and ids == [] ->
        {:noreply, socket}

      true ->
        case Chat.send(agent, body, Enum.reverse(ids)) do
          {:ok, _m} -> {:noreply, push_event(socket, "chat:sent", %{agent: agent})}
          {:error, reason} -> {:noreply, toast(socket, reason, :error)}
        end
    end
  end

  # -- teams ----------------------------------------------------------------------

  def handle_event("toggle_member", %{"agent" => name}, socket) do
    expanded = socket.assigns.expanded

    expanded =
      if MapSet.member?(expanded, name),
        do: MapSet.delete(expanded, name),
        else: MapSet.put(expanded, name)

    {:noreply, assign(socket, expanded: expanded)}
  end

  def handle_event("dissolve", %{"team" => team}, socket) do
    key = {:dissolve, team}

    cond do
      MapSet.member?(socket.assigns.dissolving, team) ->
        {:noreply, socket}

      MapSet.member?(socket.assigns.armed, key) ->
        {:noreply,
         socket
         |> update(:armed, &MapSet.delete(&1, key))
         |> update(:dissolving, &MapSet.put(&1, team))
         |> start_async({:dissolve, team}, fn -> Teams.dissolve("operator", team) end)}

      true ->
        {:noreply, arm(socket, key)}
    end
  end

  @impl true
  def handle_async({:chat_restart, name}, result, socket) do
    socket =
      case result do
        {:ok, {:ok, _}} ->
          toast(socket, "#{name} restarted with fresh context")

        _ ->
          toast(
            socket,
            "Conversation archived, but #{name} restart failed; check agent status",
            :error,
            9_000
          )
      end

    {:noreply, load(socket)}
  end

  def handle_async({:dissolve, team}, result, socket) do
    socket = update(socket, :dissolving, &MapSet.delete(&1, team))

    socket =
      case result do
        {:ok, {:ok, %{"members" => members}}} ->
          kept =
            for m <- members,
                String.starts_with?(m["worktree"] || "", "kept"),
                do: String.replace(m["worktree"], "kept (uncommitted changes): ", "")

          if kept == [],
            do: toast(socket, "Team #{team} dissolved; branches kept"),
            else:
              toast(
                socket,
                "Team #{team} dissolved. Kept: #{Enum.join(kept, ", ")}",
                :info,
                9_000
              )

        {:ok, {:error, _kind, msg}} ->
          toast(socket, msg, :error)

        {:exit, reason} ->
          toast(socket, "dissolve failed: #{inspect(reason)}", :error)
      end

    {:noreply, load(socket)}
  end

  # -- helpers ----------------------------------------------------------------------

  defp show_chat_message(m, socket) do
    # the browser decides whether to show it (only when the window is in the background)
    socket =
      if m.dir == :in do
        display = with %{display: d} <- agent(socket, m.agent), do: d, else: (_ -> m.agent)

        push_event(socket, "chat:incoming", %{
          id: m.id,
          agent: m.agent,
          title: display,
          body: String.slice(m.body, 0, 240)
        })
      else
        socket
      end

    socket =
      if socket.assigns.chat_open and m.agent == socket.assigns.chat_agent do
        socket
        |> stream_insert(:messages, m)
        |> add_unseen([m])
        |> mark_shown_read()
      else
        socket
      end

    {:noreply, load_unread(socket)}
  end

  defp select_chat(socket, nil), do: socket

  defp select_chat(socket, agent) do
    history = Chat.history(agent, @page)

    socket
    |> then(&if(agent != &1.assigns.chat_agent, do: cancel_uploads(&1), else: &1))
    |> assign(
      chat_agent: agent,
      chat_reset: nil,
      chat_oldest: history |> List.first() |> then(&(&1 && &1.id)),
      chat_more: length(history) == @page,
      chat_unseen: []
    )
    |> add_unseen(history)
    |> stream(:messages, history, reset: true)
    |> mark_shown_read()
    |> load_unread()
  end

  # images attached in one conversation never follow the operator to another
  defp cancel_uploads(socket) do
    Enum.reduce(
      socket.assigns.uploads.images.entries,
      socket,
      &cancel_upload(&2, :images, &1.ref)
    )
  end

  defp add_unseen(socket, messages) do
    ids = for m <- messages, m.dir == :in and not m.read, do: m.id
    update(socket, :chat_unseen, &(ids ++ &1))
  end

  # acks exactly the agent's messages loaded in the open conversation (never
  # older ones not loaded yet), and only while the operator can see them
  defp mark_shown_read(
         %{assigns: %{chat_open: true, visible: true, chat_agent: agent, chat_unseen: [_ | _]}} =
           socket
       )
       when is_binary(agent) do
    Chat.mark_read(agent, ids: socket.assigns.chat_unseen)
    assign(socket, chat_unseen: [])
  end

  defp mark_shown_read(socket), do: socket

  defp upload_error(:too_large), do: "over 10 MB"
  defp upload_error(:bad_type), do: "only PNG, JPEG, WebP or GIF"
  defp upload_error(:not_accepted), do: "only PNG, JPEG, WebP or GIF"
  defp upload_error(:too_many_files), do: "at most 6 images"
  defp upload_error(other), do: to_string(other)

  # launches and kills take seconds (tmux, Codex app-server): run them aside
  # and let the next ticks show the result
  defp lifecycle(socket, name, fun, verb) do
    case agent(socket, name) do
      nil ->
        {:noreply, toast(socket, "unknown agent", :error)}

      a ->
        Task.start(fn -> fun.(name) end)
        {:noreply, toast(socket, "#{verb} #{a.display}")}
    end
  end

  defp arm(socket, key) do
    Process.send_after(self(), {:disarm, key}, @arm_ms)
    update(socket, :armed, &MapSet.put(&1, key))
  end

  defp toast(socket, text, kind \\ :info, ms \\ @toast_ms) do
    ref = make_ref()
    Process.send_after(self(), {:clear_toast, ref}, ms)
    assign(socket, toast: %{text: text, kind: kind, ref: ref})
  end

  defp agent(socket, name), do: Enum.find(socket.assigns.snap.agents, &(&1.name == name))

  defp draft(%Phoenix.LiveView.Socket{} = socket, a), do: draft(socket.assigns.drafts, a)

  defp draft(drafts, a),
    do: Map.get(drafts, a.name, %{harness: a.harness, model: a.model || "", custom: false})

  defp put_draft(socket, a, d) do
    if dirty?(a, d) or d.custom,
      do: update(socket, :drafts, &Map.put(&1, a.name, d)),
      else: update(socket, :drafts, &Map.delete(&1, a.name))
  end

  # an empty "Other…" field is not a model to apply
  defp dirty?(a, d),
    do:
      not (d.custom and d.model == "") and (d.harness != a.harness or d.model != (a.model || ""))

  defp first_model(socket, a, harness) do
    if harness == a.harness,
      do: a.model || "",
      else: List.first(model_list(socket.assigns.snap.models, a, harness)) || ""
  end

  # a harness's known models, plus the agent's current model if it isn't one of
  # them (only on its own harness); "" stands for the harness default
  defp model_list(models, a, harness) do
    list = Map.get(models, harness, [])

    cond do
      harness != a.harness -> list
      is_nil(a.model) -> ["" | list]
      a.model in list -> list
      true -> [a.model | list]
    end
  end

  # -- render -----------------------------------------------------------------------

  @impl true
  def render(assigns) do
    assigns =
      assign(assigns,
        coordinators: Enum.filter(assigns.snap.agents, &is_nil(&1.team)),
        by_name: Map.new(assigns.snap.agents, &{&1.name, &1})
      )

    ~H"""
    <header>
      <h1>Aperture</h1>
      <span class="live"><span class="on-text">live</span><span class="off-text">connecting…</span></span>
      <span class="hint">click an agent to jump to it in <code>tmux attach -t aperture-v4</code></span>
      <span id="notifier" phx-hook="Notifier" phx-update="ignore">
        <button type="button" class="ghost" hidden title="Get a macOS notification when an agent writes to you">
          Enable notifications
        </button>
      </span>
      <button id="chat-button" class="chat-button" phx-click="chat_open">
        Chat
        <span :if={total(@unread) > 0} class="badge">{total(@unread)}</span>
      </button>
    </header>
    <main>
      <h2>Agents</h2>
      <div class="agents" id="agents">
        <.card
          :for={a <- @coordinators}
          a={a}
          draft={draft(@drafts, a)}
          snap={@snap}
          unread={Map.get(@unread, a.name, 0)}
          armed={MapSet.member?(@armed, {:stop, a.name})}
        />
      </div>
      <section :for={t <- @snap.teams} class="team-panel" id={"team-#{t.name}"}>
        <header class="team-bar">
          <span class="team-mark" aria-hidden="true">▣</span>
          <h2>Team {t.name}</h2>
          <span class="team-meta">
            <span class="mono" title={t.project}>{home(t.project)}</span>
            <span :if={t.base}>· base <span class="mono">{t.base}</span></span>
            <span>· lead {t.lead}</span>
            <span>· {length(t.roster)} member{if length(t.roster) == 1, do: "", else: "s"}</span>
          </span>
          <button
            :if={t.roster != []}
            type="button"
            class="ghost"
            phx-click="chat_open"
            phx-value-agent={hd(t.roster).name}
          >
            Chat
          </button>
          <button
            class={["danger", MapSet.member?(@armed, {:dissolve, t.name}) && "armed"]}
            phx-click="dissolve"
            phx-value-team={t.name}
            disabled={MapSet.member?(@dissolving, t.name)}
            title="Stop the members; clean worktrees are removed, branches kept"
          >
            {dissolve_label(@armed, @dissolving, t.name)}
          </button>
        </header>
        <div class="roster" role="table" aria-label={"Team #{t.name} members"}>
          <.member
            :for={m <- t.roster}
            :if={@by_name[m.name]}
            m={m}
            a={@by_name[m.name]}
            task={@tasks[m.name]}
            unread={Map.get(@unread, m.name, 0)}
            open={MapSet.member?(@expanded, m.name)}
            draft={draft(@drafts, @by_name[m.name])}
            snap={@snap}
            armed={MapSet.member?(@armed, {:stop, m.name})}
          />
        </div>
      </section>
    </main>
    <.chat
      :if={@chat_open}
      agents={@snap.agents}
      teams={@snap.teams}
      agent={@by_name[@chat_agent]}
      unread={@unread}
      streams={@streams}
      uploads={@uploads}
      more={@chat_more}
      resetting={@chat_reset}
    />
    <div :if={@toast} class={["toast", "show", @toast.kind == :error && "error"]} id="toast">
      {@toast.text}
    </div>
    """
  end

  defp dissolve_label(armed, dissolving, team) do
    cond do
      MapSet.member?(dissolving, team) -> "Dissolving…"
      MapSet.member?(armed, {:dissolve, team}) -> "Confirm dissolve"
      true -> "Dissolve"
    end
  end

  defp home(path) do
    home = System.user_home!()

    if String.starts_with?(path, home <> "/"),
      do: "~" <> String.replace_prefix(path, home, ""),
      else: path
  end

  # a coordinator: a card
  attr(:a, :map, required: true)
  attr(:draft, :map, required: true)
  attr(:snap, :map, required: true)
  attr(:unread, :integer, default: 0)
  attr(:armed, :boolean, default: false)

  defp card(assigns) do
    ~H"""
    <div class={["card", @a.focused && "focused"]} id={"agent-#{@a.name}"} data-agent={@a.name}>
      <div class="top">
        <div class="who" title="Switch tmux to this agent" phx-click="focus" phx-value-agent={@a.name}>
          <span class={["dot", @a.state]}></span>
          <div>
            <span class="name">{@a.display}</span>
            <span class="state">{@a.state} · {@a.harness} · {@a.model || "default"}</span>
          </div>
        </div>
        <.chat_button a={@a} unread={@unread} />
      </div>
      <p class="role">{@a.role}</p>
      <.controls a={@a} draft={@draft} snap={@snap} armed={@armed} />
    </div>
    """
  end

  # a team member: one row of the team's table; ⋯ opens the same controls
  attr(:m, :map, required: true)
  attr(:a, :map, required: true)
  attr(:task, :map, default: nil)
  attr(:unread, :integer, default: 0)
  attr(:open, :boolean, default: false)
  attr(:draft, :map, required: true)
  attr(:snap, :map, required: true)
  attr(:armed, :boolean, default: false)

  defp member(assigns) do
    ~H"""
    <div
      class={["member", @a.focused && "focused", @open && "open"]}
      id={"member-#{@a.name}"}
      data-agent={@a.name}
      role="rowgroup"
    >
      <div class="member-row" role="row">
        <span class={["dot", @a.state]} title={to_string(@a.state)}></span>
        <span
          class="m-role"
          role="cell"
          title="Switch tmux to this agent"
          phx-click="focus"
          phx-value-agent={@a.name}
        >
          {@m.role}
        </span>
        <span class="m-model" role="cell">{@a.harness} · {@a.model || "default"}</span>
        <span class="m-branch mono" role="cell" title={@m.branch || "works in the project directory"}>
          <%= if @m.branch do %>
            ⎇ {@m.branch}
          <% else %>
            <span class="muted">project dir</span>
          <% end %>
        </span>
        <span class="m-task" role="cell" title={@task && "#{@task.id} · #{@task.title}"}>
          <%= if @task do %>
            <span class="mono task-id">{@task.id}</span> {@task.title}
          <% else %>
            <span class="muted">—</span>
          <% end %>
        </span>
        <span class={["m-state", @a.state]} role="cell">{@a.state}</span>
        <.chat_button a={@a} unread={@unread} />
        <button
          type="button"
          class="ghost more"
          phx-click="toggle_member"
          phx-value-agent={@a.name}
          aria-expanded={to_string(@open)}
          title="Harness, model, restart, stop"
        >
          ⋯
        </button>
      </div>
      <div :if={@open} class="member-controls">
        <.controls a={@a} draft={@draft} snap={@snap} armed={@armed} />
      </div>
    </div>
    """
  end

  attr(:a, :map, required: true)
  attr(:unread, :integer, default: 0)

  defp chat_button(assigns) do
    ~H"""
    <button
      type="button"
      class={["ghost", "chat-open", @unread > 0 && "has-unread"]}
      phx-click="chat_open"
      phx-value-agent={@a.name}
      title={
        if @unread > 0,
          do: "#{@unread} unread message#{if @unread == 1, do: "", else: "s"} for you",
          else: "Chat with #{@a.display}"
      }
    >
      <span :if={@unread > 0} class="badge">{@unread}</span>
      <span :if={@unread == 0}>Chat</span>
    </button>
    """
  end

  # harness / model (with "Other…") / Apply, Restart, Stop|Start
  attr(:a, :map, required: true)
  attr(:draft, :map, required: true)
  attr(:snap, :map, required: true)
  attr(:armed, :boolean, default: false)

  defp controls(assigns) do
    a = assigns.a
    d = assigns.draft

    assigns =
      assign(assigns,
        models: model_list(assigns.snap.models, a, d.harness),
        dirty: dirty?(a, d),
        offline: a.state in [:offline, :startup_failed]
      )

    ~H"""
    <form id={"cfg-#{@a.name}"} class="cfg" phx-change="config_change" phx-submit="apply">
      <input type="hidden" name="agent" value={@a.name} />
      <div class="fields">
        <label for={"harness-#{@a.name}"}>Harness</label>
        <select id={"harness-#{@a.name}"} name="harness" aria-label="harness">
          <option :for={h <- @snap.harnesses} value={h} selected={h == @draft.harness}>{h}</option>
        </select>
        <label for={"model-#{@a.name}"}>Model</label>
        <div>
          <input
            :if={@draft.custom}
            id={"custom-#{@a.name}"}
            name="custom"
            value={@draft.model}
            aria-label="custom model id"
            placeholder="model id (Esc: list)"
            phx-keydown="custom_escape"
            phx-key="Escape"
            phx-value-agent={@a.name}
            phx-mounted={Phoenix.LiveView.JS.focus()}
            autocomplete="off"
          />
          <select :if={!@draft.custom} id={"model-#{@a.name}"} name="model" aria-label="model">
            <option :for={m <- @models} value={m} selected={m == @draft.model}>
              {if m == "", do: "default", else: m}
            </option>
            <option value={other()}>Other…</option>
          </select>
        </div>
      </div>
      <div class="actions">
        <button type="submit" class="primary" disabled={!@dirty} title="Save and restart on this harness/model">
          Apply
        </button>
        <span class="spacer"></span>
        <button type="button" phx-click="restart" phx-value-agent={@a.name} title="Kill and relaunch this agent">
          Restart
        </button>
        <button
          type="button"
          class={["power", !@offline && "danger", @armed && "armed"]}
          phx-click="power"
          phx-value-agent={@a.name}
          title={if @offline, do: "Launch this agent", else: "Stop this agent (kills its session)"}
        >
          {cond do
            @offline -> "Start"
            @armed -> "Confirm stop"
            true -> "Stop"
          end}
        </button>
      </div>
    </form>
    """
  end

  defp other, do: @other

  defp total(unread), do: unread |> Map.values() |> Enum.sum()

  # -- chat modal -------------------------------------------------------------------

  attr(:agents, :list, required: true)
  attr(:teams, :list, required: true)
  attr(:agent, :map, default: nil)
  attr(:unread, :map, required: true)
  attr(:streams, :any, required: true)
  attr(:uploads, :any, required: true)
  attr(:more, :boolean, default: false)
  attr(:resetting, :string, default: nil)

  defp chat(assigns) do
    groups =
      [{"Agents", Enum.filter(assigns.agents, &is_nil(&1.team))}] ++
        for t <- assigns.teams,
            members = Enum.filter(assigns.agents, &(&1.team == t.name)),
            members != [],
            do: {"Team #{t.name}", members}

    assigns = assign(assigns, groups: groups)

    ~H"""
    <div class="modal-backdrop">
      <div
        id="chat"
        class="modal chat"
        role="dialog"
        aria-label="Chat"
        phx-click-away="chat_close"
        phx-window-keydown="chat_close"
        phx-key="Escape"
      >
        <nav class="convos">
          <div :for={{label, members} <- @groups} class="convo-group">
            <h2>{label}</h2>
            <button
              :for={a <- members}
              type="button"
              id={"convo-#{a.name}"}
              class={["convo", @agent && @agent.name == a.name && "active"]}
              phx-click="chat_select"
              phx-value-agent={a.name}
            >
              <span class={["dot", a.state]}></span>
              <span class="convo-name">{a.display}</span>
              <span :if={Map.get(@unread, a.name, 0) > 0} class="badge">{@unread[a.name]}</span>
            </button>
          </div>
        </nav>
        <section class="thread" :if={@agent}>
          <header class="thread-head">
            <span class={["dot", @agent.state]}></span>
            <strong>{@agent.display}</strong>
            <span class="state">{@agent.state} · {@agent.harness} · {@agent.model || "default"}</span>
            <span :if={@agent.team} class="note">reports to its lead; replies come through them</span>
            <button id="chat-reset" type="button" class="ghost" phx-click="chat_reset">Reset</button>
            <button type="button" class="ghost close" phx-click="chat_close" title="Close (Esc)">✕</button>
          </header>
          <form :if={@resetting == @agent.name} id="chat-reset-confirm" class="reset-confirm"
            phx-submit="chat_reset_confirm" aria-label="Confirm conversation reset">
            <input type="hidden" name="agent" value={@agent.name} />
            <p>Reset the conversation with <strong>{@agent.display}</strong>?</p>
            <p>History will be archived, not deleted. Pending messages still deliver.</p>
            <label><input type="checkbox" name="restart" value="true" />
              Also restart agent with fresh context (interrupts current work)
            </label>
            <div>
              <button type="submit" class="primary" phx-disable-with="Resetting…">Confirm reset</button>
              <button id="chat-reset-cancel" type="button" class="ghost" phx-click="chat_reset_cancel">Cancel</button>
            </div>
          </form>
          <div id="chat-scroll" class="history" phx-hook="ChatScroll">
            <button :if={@more} id="chat-older" type="button" class="ghost older" phx-click="chat_older">
              Older messages
            </button>
            <div id="chat-history" class="history-list" phx-update="stream">
              <div id="chat-empty" class="empty only-child">No messages yet.</div>
              <div :for={{id, m} <- @streams.messages} id={id} class={["bubble", m.dir]}>
                <div :if={m.body != ""} class="body">{m.body}</div>
                <a :for={att <- m.attachments} href={att.url} target="_blank" class="shot">
                  <img src={att.url} alt="image" loading="lazy" />
                </a>
                <time datetime={m.at} title={m.at}>{clock(m.at)}</time>
              </div>
            </div>
          </div>
          <form
            id="chat-form"
            class="composer"
            phx-submit="chat_send"
            phx-change="chat_validate"
            phx-drop-target={@uploads.images.ref}
          >
            <input type="hidden" name="agent" value={@agent.name} />
            <div :if={@uploads.images.entries != []} class="previews">
              <figure :for={e <- @uploads.images.entries} class="preview">
                <.live_img_preview entry={e} />
                <button
                  type="button"
                  class="remove"
                  phx-click="chat_cancel_upload"
                  phx-value-ref={e.ref}
                  aria-label="remove image"
                >
                  ✕
                </button>
                <figcaption :for={err <- upload_errors(@uploads.images, e)} class="error">
                  {upload_error(err)}
                </figcaption>
              </figure>
            </div>
            <p :for={err <- upload_errors(@uploads.images)} class="error">{upload_error(err)}</p>
            <div class="compose-row">
              <label class="attach" title="Attach images">
                ＋ <.live_file_input upload={@uploads.images} />
              </label>
              <textarea
                id="chat-input"
                name="body"
                rows="1"
                phx-hook="ChatInput"
                phx-update="ignore"
                data-agent={@agent.name}
                placeholder={"Message #{@agent.display}… (Enter sends, Shift+Enter new line, ⌘V pastes an image)"}
              ></textarea>
              <button type="submit" class="primary">Send</button>
            </div>
          </form>
        </section>
      </div>
    </div>
    """
  end

  # stored UTC -> this machine's local time ("22:54"; "06/10 22:54" if not today)
  defp clock(at) do
    with {:ok, dt, _} <- DateTime.from_iso8601(at) do
      utc =
        dt
        |> DateTime.to_naive()
        |> NaiveDateTime.to_erl()
        |> then(fn {d, {h, m, s}} -> {d, {h, m, trunc(s)}} end)

      {{y, mo, d}, {h, mi, _}} = :calendar.universal_time_to_local_time(utc)
      {today, _} = :calendar.local_time()
      hm = :io_lib.format("~2..0B:~2..0B", [h, mi]) |> IO.iodata_to_binary()

      if {y, mo, d} == today,
        do: hm,
        else: (:io_lib.format("~2..0B/~2..0B ", [d, mo]) |> IO.iodata_to_binary()) <> hm
    else
      _ -> at
    end
  end
end
