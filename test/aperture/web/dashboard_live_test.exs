defmodule Aperture.Web.DashboardLiveTest do
  @moduledoc """
  The launcher page (LiveView) against the real hub: agents defined on disk,
  launched in real tmux with the fake `claude` (which runs the real inbox), so
  Apply / Restart / Stop / Start / Dissolve are checked by their effect.
  """
  use ExUnit.Case, async: false

  import Phoenix.ConnTest
  import Phoenix.LiveViewTest

  alias Aperture.{AgentFixtures, Agents, FakeAgent, Launcher, Presence, Tmux}

  @endpoint Aperture.Web.Endpoint

  @moduletag skip:
               if(
                 System.find_executable("tmux") && System.find_executable("node") &&
                   System.find_executable("git"),
                 do: false,
                 else: "needs tmux, node and git"
               )

  setup_all do
    on_exit(fn -> Tmux.kill_session(Application.fetch_env!(:aperture, :tmux_session)) end)
  end

  defp page, do: live(build_conn(), "/")
  defp wait(fun, msg, timeout \\ 10_000), do: FakeAgent.wait_until(fun, msg, timeout)

  defp started(prefix, attrs \\ %{}) do
    name = AgentFixtures.define(AgentFixtures.unique(prefix), attrs)
    {:ok, _} = Launcher.start_agent(name)
    on_exit(fn -> Launcher.stop_agent(name) end)
    wait(fn -> Presence.state(name) != :offline end, "#{name} never online")
    name
  end

  # fake-claude records its argv on every launch
  defp args_file(name),
    do:
      Path.join(
        Application.fetch_env!(:aperture, :launch_env)["FAKE_LOG_DIR"],
        "claude-#{name}.args"
      )

  defp wait_relaunch(name) do
    wait(fn -> File.exists?(args_file(name)) end, "#{name} not relaunched", 15_000)
    File.read!(args_file(name)) |> String.split(<<0>>, trim: true)
  end

  # re-render after the page's next tick has picked up the change
  defp eventually(view, fun, msg) do
    wait(
      fn ->
        send(view.pid, :tick)
        fun.(render(view))
      end,
      msg
    )
  end

  test "lists every agent with its live state, harness and model" do
    name =
      AgentFixtures.define(AgentFixtures.unique("l"), %{
        "harness" => "codex",
        "model" => "gpt-6-astra"
      })

    {:ok, view, html} = page()

    assert html =~ ~s(id="agent-#{name}")
    assert view |> element("#agent-#{name} .state") |> render() =~ "offline · codex · gpt-6-astra"

    _ = FakeAgent.start(name)
    eventually(view, &(&1 =~ "idle · codex · gpt-6-astra"), "never idle")
  end

  test "Apply saves the harness and model and restarts the agent on them" do
    name = started("a")
    {:ok, view, _} = page()
    form = "#cfg-#{name}"

    assert view |> element("#{form} button[type=submit]") |> render() =~ "disabled"

    # switching harness picks that harness's first model
    render_change(view, "config_change", %{"agent" => name, "harness" => "codex"})
    assert view |> element("#model-#{name} option[selected]") |> render() =~ "gpt-6-astra"

    # back on its own harness: its current model again, nothing to apply
    render_change(view, "config_change", %{"agent" => name, "harness" => "claude"})
    assert view |> element("#model-#{name} option[selected]") |> render() =~ "opus"
    assert view |> element("#{form} button[type=submit]") |> render() =~ "disabled"

    render_change(view, "config_change", %{
      "agent" => name,
      "harness" => "claude",
      "model" => "sonnet"
    })

    refute view |> element("#{form} button[type=submit]") |> render() =~ "disabled"

    # an edit in progress survives live refreshes
    send(view.pid, :tick)
    assert view |> element("#model-#{name} option[selected]") |> render() =~ "sonnet"

    File.rm!(args_file(name))
    assert view |> form(form) |> render_submit() =~ "Restarting"
    assert %{harness: "claude", model: "sonnet"} = Agents.get(name)
    args = wait_relaunch(name)
    assert Enum.at(args, Enum.find_index(args, &(&1 == "--model")) + 1) == "sonnet"
  end

  test "Other… takes any model id; Esc goes back to the list" do
    name = AgentFixtures.define(AgentFixtures.unique("c"))
    {:ok, view, _} = page()

    render_change(view, "config_change", %{
      "agent" => name,
      "harness" => "claude",
      "model" => "__other__"
    })

    assert has_element?(view, "#custom-#{name}")
    # nothing typed yet: nothing to apply
    assert view |> element("#cfg-#{name} button[type=submit]") |> render() =~ "disabled"

    render_keydown(view |> element("#custom-#{name}"), %{"key" => "Escape"})
    refute has_element?(view, "#custom-#{name}")
    assert has_element?(view, "#model-#{name}")

    render_change(view, "config_change", %{
      "agent" => name,
      "harness" => "claude",
      "model" => "__other__"
    })

    render_change(view, "config_change", %{
      "agent" => name,
      "harness" => "claude",
      "custom" => "claude-x-1"
    })

    view |> form("#cfg-#{name}") |> render_submit()
    assert %{model: "claude-x-1"} = Agents.get(name)
  end

  test "Restart relaunches; Stop needs a confirming click; Start brings it back" do
    name = started("s")
    {:ok, view, _} = page()
    power = "#agent-#{name} button[phx-click=power]"

    File.rm!(args_file(name))

    assert view |> element("#agent-#{name} button[phx-click=restart]") |> render_click() =~
             "Restarting"

    wait_relaunch(name)
    wait(fn -> Presence.state(name) != :offline end, "not back after restart")

    assert view |> element(power) |> render_click() =~ "Confirm stop"
    assert Presence.state(name) != :offline

    view |> element(power) |> render_click()
    wait(fn -> Presence.state(name) == :offline end, "still online")
    eventually(view, &(&1 =~ ~r/phx-value-agent="#{name}"[^>]*>\s*Start/), "no Start button")

    view |> element(power) |> render_click()
    wait(fn -> Presence.state(name) != :offline end, "never back")
  end

  test "an armed Stop disarms itself" do
    name = started("d")
    {:ok, view, _} = page()
    power = "#agent-#{name} button[phx-click=power]"

    assert view |> element(power) |> render_click() =~ "Confirm stop"
    send(view.pid, {:disarm, {:stop, name}})
    refute render(view) =~ "Confirm stop"
    assert Presence.state(name) != :offline
  end

  test "clicking an agent focuses its tmux window" do
    a = started("f")
    b = started("g")
    session = Application.fetch_env!(:aperture, :tmux_session)
    {:ok, view, _} = page()

    view |> element("#agent-#{a} .who") |> render_click()
    assert Tmux.current_window(session) == a
    view |> element("#agent-#{b} .who") |> render_click()
    assert Tmux.current_window(session) == b
    eventually(view, &(&1 =~ ~r/class="card focused"[^>]*id="agent-#{b}"/), "not highlighted")
  end

  describe "chat" do
    @png <<0x89, "PNG", 0x0D, 0x0A, 0x1A, 0x0A, "rest of a png">>

    # the browser reports the page visible and focused; until it does, nothing is read
    defp open_chat(view, name) do
      render_hook(view, "visibility", %{"visible" => true})
      view |> element("#agent-#{name} .chat-open") |> render_click()
      assert has_element?(view, "#chat")
    end

    test "reset requires confirmation, archives just this conversation and updates other tabs" do
      name = AgentFixtures.define(AgentFixtures.unique("reset"))
      other = AgentFixtures.define(AgentFixtures.unique("keep"))
      fa = FakeAgent.start(name)
      {:ok, old} = Aperture.Store.insert(name, "operator", "old reset history")
      {:ok, kept} = Aperture.Store.insert(other, "operator", "other history")
      {:ok, view, _} = page()
      {:ok, tab, _} = page()
      open_chat(view, name)
      open_chat(tab, name)

      # A forged/stale confirm without first opening the confirmation cannot reset.
      render_submit(view, "chat_reset_confirm", %{"agent" => name})
      assert has_element?(view, "#messages-#{old.id}")
      view |> element("#chat-reset") |> render_click()
      assert has_element?(view, "#chat-reset-confirm")
      assert has_element?(view, "#messages-#{old.id}")
      view |> element("#chat-reset-cancel") |> render_click()
      refute has_element?(view, "#chat-reset-confirm")
      assert Aperture.Chat.history(name) != []

      view |> element("#chat-reset") |> render_click()
      view |> form("#chat-reset-confirm") |> render_submit()
      refute has_element?(view, "#messages-#{old.id}")
      wait(fn -> not has_element?(tab, "#messages-#{old.id}") end, "other tab kept old messages")
      assert Aperture.Store.get(old.id).body == "old reset history"
      assert Aperture.Store.get(kept.id).body == "other history"
      assert Aperture.Chat.history(name) == []
      assert [%{id: id}] = Aperture.Chat.history(other)
      assert id == kept.id

      # Late PubSub from before the reset may not resurrect archived content.
      send(view.pid, {:chat_message, %{id: old.id, agent: name, dir: :in, body: old.body}})
      refute render(view) =~ "old reset history"
      view |> element("button[phx-click=chat_close]") |> render_click()
      open_chat(view, name)
      refute render(view) =~ "old reset history"
      {:ok, refreshed, _} = live(build_conn(), "/?chat=#{name}")
      refute render(refreshed) =~ "old reset history"

      FakeAgent.send_message(fa, "operator", "new after reset")
      wait(fn -> render(view) =~ "new after reset" end, "new message missing")
      view |> form("#chat-form", %{"body" => "new outgoing"}) |> render_submit()
      assert FakeAgent.wait_for(fa, 1) == ["new outgoing"]

      assert Enum.map(Aperture.Chat.history(name), & &1.body) == [
               "new after reset",
               "new outgoing"
             ]
    end

    test "reset confirmation is scoped to its conversation and cancelled on switching or closing" do
      a = AgentFixtures.define(AgentFixtures.unique("ra"))
      b = AgentFixtures.define(AgentFixtures.unique("rb"))
      Aperture.Store.insert(a, "operator", "keep a")
      Aperture.Store.insert(b, "operator", "keep b")
      {:ok, view, _} = page()
      open_chat(view, a)
      view |> element("#chat-reset") |> render_click()
      view |> element("#convo-#{b}") |> render_click()
      refute has_element?(view, "#chat-reset-confirm")
      render_submit(view, "chat_reset_confirm", %{"agent" => a, "restart" => "true"})
      assert length(Aperture.Chat.history(a)) == 1
      assert length(Aperture.Chat.history(b)) == 1
      view |> element("#chat-reset") |> render_click()
      view |> element("button[phx-click=chat_close]") |> render_click()
      open_chat(view, b)
      refute has_element?(view, "#chat-reset-confirm")
    end

    test "reset leaves the agent running unless restart is checked, then actually relaunches" do
      name = started("rr")
      {:ok, view, _} = page()
      open_chat(view, name)
      File.rm!(args_file(name))
      view |> element("#chat-reset") |> render_click()
      view |> form("#chat-reset-confirm") |> render_submit()
      render_async(view)
      refute File.exists?(args_file(name))
      assert Presence.state(name) != :offline

      view |> element("#chat-reset") |> render_click()
      view |> form("#chat-reset-confirm", %{"restart" => "true"}) |> render_submit()
      assert render_async(view, 5_000) =~ "restarted with fresh context"
      args = wait_relaunch(name)
      refute "--resume" in args
      refute "--continue" in args
    end

    test "the operator sends with the composer; the agent gets it" do
      name = AgentFixtures.define(AgentFixtures.unique("m"))
      fa = FakeAgent.start(name)
      {:ok, view, _} = page()
      open_chat(view, name)
      assert has_element?(view, "#convo-#{name}.active")

      view |> form("#chat-form", %{"body" => "deploy it, please"}) |> render_submit()
      assert FakeAgent.wait_for(fa, 1) == ["deploy it, please"]
      assert render(view) =~ ~r/class="bubble out".*deploy it, please/s

      # blank: nothing is sent
      view |> form("#chat-form", %{"body" => "  "}) |> render_submit()
      Process.sleep(100)
      assert length(FakeAgent.received(fa)) == 1
    end

    test "an agent's message to the operator shows up live, with a badge until read" do
      name = AgentFixtures.define(AgentFixtures.unique("r"))
      other = AgentFixtures.define(AgentFixtures.unique("q"))
      fa = FakeAgent.start(name)
      {:ok, view, _} = page()

      # other tests' messages may be unread too: count from here
      total = Aperture.Chat.unread_total() + 1

      FakeAgent.send_message(fa, "operator", "need your decision on the deploy")
      wait(fn -> render(view) =~ ~r/id="agent-#{name}".*class="badge">1</s end, "no card badge")
      assert view |> element("#chat-button .badge") |> render() =~ ">#{total}<"
      assert page_title(view) =~ "(#{total}) Aperture"

      # reading it in the chat clears the badge
      open_chat(view, name)
      assert render(view) =~ "need your decision on the deploy"
      assert Aperture.Chat.unread_counts()[name] == nil
      refute has_element?(view, "#agent-#{name} .badge")

      # while open on this agent, new messages arrive already read
      FakeAgent.send_message(fa, "operator", "second thought")
      wait(fn -> render(view) =~ "second thought" end, "not shown live")
      assert Aperture.Chat.unread_counts()[name] == nil

      # on another conversation, they stay unread
      view |> element("#convo-#{other}") |> render_click()
      FakeAgent.send_message(fa, "operator", "third")
      wait(fn -> has_element?(view, "#convo-#{name} .badge") end, "no list badge")
      refute render(view) =~ "third"

      render_keydown(view |> element("#chat"), %{"key" => "Escape"})
      refute has_element?(view, "#chat")
      Aperture.Chat.mark_read(name)
    end

    test "an image is previewed, then sent: stored, shown in the history, path given to the agent" do
      name = AgentFixtures.define(AgentFixtures.unique("i"))
      fa = FakeAgent.start(name)
      {:ok, view, _} = page()
      open_chat(view, name)

      image =
        file_input(view, "#chat-form", :images, [
          %{name: "shot.png", content: @png, type: "image/png"}
        ])

      assert render_upload(image, "shot.png") =~ ~s(class="preview")

      view |> form("#chat-form", %{"body" => "look"}) |> render_submit()
      [text] = FakeAgent.wait_for(fa, 1)
      [_, path] = Regex.run(~r/\[image attached: (\S+)\]/, text)
      assert text =~ ~r/^look\n\n/
      assert File.read!(path) == @png

      html = render(view)
      refute html =~ ~s(class="preview")
      assert html =~ ~s(src="/uploads/#{Path.basename(path)}")
    end

    test "switching conversations never carries A's images or text to B" do
      a = AgentFixtures.define(AgentFixtures.unique("ya"))
      b = AgentFixtures.define(AgentFixtures.unique("yb"))
      fa = FakeAgent.start(a)
      fb = FakeAgent.start(b)
      {:ok, view, _} = page()
      open_chat(view, a)

      image =
        file_input(view, "#chat-form", :images, [
          %{name: "for-a.png", content: @png, type: "image/png"}
        ])

      render_upload(image, "for-a.png")
      assert render(view) =~ ~s(class="preview")

      view |> element("#convo-#{b}") |> render_click()
      refute render(view) =~ ~s(class="preview")

      view |> form("#chat-form", %{"body" => "only text for b"}) |> render_submit()
      assert FakeAgent.wait_for(fb, 1) == ["only text for b"]
      assert [%{attachments: []}] = Aperture.Chat.history(b)

      # a submit composed for A (sent while B was being selected) is not delivered to B
      render_submit(view, "chat_send", %{"agent" => a, "body" => "typed for a"})
      Process.sleep(100)
      assert length(FakeAgent.received(fb)) == 1
      assert FakeAgent.received(fa) == []
    end

    test "a long backlog: only what is loaded is marked read; older pages load on demand" do
      name = AgentFixtures.define(AgentFixtures.unique("bl"))
      for i <- 1..205, do: Aperture.Store.insert(name, "operator", "old #{i}")
      on_exit(fn -> Aperture.Chat.mark_read(name) end)
      {:ok, view, _} = page()

      open_chat(view, name)
      html = render(view)
      assert html =~ "old 205"
      refute html =~ ~r/>old 5</
      # the 5 not shown are still unread
      assert Aperture.Chat.unread_counts()[name] == 5

      view |> element("#chat-older") |> render_click()
      assert render(view) =~ ~r/>old 1</
      assert Aperture.Chat.unread_counts()[name] == nil
      refute has_element?(view, "#chat-older")
    end

    test "with the window in the background, an open conversation does not mark new messages read" do
      name = AgentFixtures.define(AgentFixtures.unique("bg"))
      fa = FakeAgent.start(name)
      {:ok, view, _} = page()
      open_chat(view, name)

      render_hook(view, "visibility", %{"visible" => false})
      FakeAgent.send_message(fa, "operator", "while you were away")
      wait(fn -> render(view) =~ "while you were away" end, "not shown")
      assert Aperture.Chat.unread_counts()[name] == 1
      assert view |> element("#chat-button .badge") |> render() =~ ">"

      # back in front, on that conversation: now it is read
      render_hook(view, "visibility", %{"visible" => true})
      assert Aperture.Chat.unread_counts()[name] == nil
    end

    test "an image that is not really an image is refused" do
      name = AgentFixtures.define(AgentFixtures.unique("x"))
      fa = FakeAgent.start(name)
      {:ok, view, _} = page()
      open_chat(view, name)

      fake =
        file_input(view, "#chat-form", :images, [
          %{name: "x.png", content: "#!/bin/sh\n", type: "image/png"}
        ])

      render_upload(fake, "x.png")
      assert view |> form("#chat-form", %{"body" => "x"}) |> render_submit() =~ "image refused"
      Process.sleep(100)
      assert FakeAgent.received(fa) == []
    end
  end

  describe "installed app (PWA)" do
    test "an agent's message is pushed to the browser for a notification, with the unread count for the badge" do
      name = AgentFixtures.define(AgentFixtures.unique("n"), %{"display" => "Nina"})
      fa = FakeAgent.start(name)
      {:ok, view, _} = page()
      on_exit(fn -> Aperture.Chat.mark_read(name) end)
      total = Aperture.Chat.unread_total() + 1

      FakeAgent.send_message(fa, "operator", "build is green")

      assert_push_event(
        view,
        "chat:incoming",
        %{agent: ^name, title: "Nina", body: "build is green"},
        5_000
      )

      assert_push_event(view, "unread", %{total: ^total}, 5_000)
    end

    test "/?chat=<agent> opens that conversation (a notification clicked with no window open)" do
      name = AgentFixtures.define(AgentFixtures.unique("p"))
      Aperture.Store.insert(name, "operator", "waiting for you")
      on_exit(fn -> Aperture.Chat.mark_read(name) end)

      # the plain HTTP render (no JS, no focus known) reads nothing
      conn = get(build_conn(), "/?chat=#{name}")
      assert html_response(conn, 200) =~ "waiting for you"
      assert Aperture.Chat.unread_counts()[name] == 1

      # nor does the connected page until the browser says it is in front
      {:ok, view, _} = live(build_conn(), "/?chat=#{name}")
      assert has_element?(view, "#convo-#{name}.active")
      assert Aperture.Chat.unread_counts()[name] == 1

      render_hook(view, "visibility", %{"visible" => true})
      assert Aperture.Chat.unread_counts()[name] == nil

      {:ok, view, _} = live(build_conn(), "/?chat=nobody")
      refute has_element?(view, "#chat")
    end
  end

  describe "teams" do
    test "current tasks come from BEADS (in_progress, by assignee) and refresh off the page's clock" do
      assert Aperture.Web.TeamTasks.index([
               %{
                 "id" => "a-1",
                 "title" => "Doing",
                 "status" => "in_progress",
                 "assignee" => "x-dev"
               },
               %{"id" => "a-2", "title" => "Later", "status" => "open", "assignee" => "x-qa"},
               %{"id" => "a-3", "title" => "Nobody", "status" => "in_progress", "assignee" => ""}
             ]) == %{"x-dev" => %{id: "a-1", title: "Doing"}}

      # through the real Beads module and the fake bd
      :ok = Aperture.Web.TeamTasks.refresh()
      assert %{"peppy" => %{id: "ap-2", title: "Second"}} = Aperture.Web.TeamTasks.current()
    end

    defp git_repo do
      dir =
        Path.join(System.tmp_dir!(), "aperture-test-repo-#{System.unique_integer([:positive])}")

      File.mkdir_p!(dir)
      git = &System.cmd("git", ["-C", dir | &1], stderr_to_stdout: true)
      {_, 0} = git.(["init", "-q"])
      File.write!(Path.join(dir, "README.md"), "hello\n")
      {_, 0} = git.(["add", "."])
      {_, 0} = git.(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"])
      on_exit(fn -> File.rm_rf(dir) end)
      dir
    end

    test "a team shows its members; Dissolve (confirmed) stops them and removes the team" do
      repo = git_repo()
      team = "t#{System.unique_integer([:positive])}"
      member = "#{team}-dev"

      {:ok, _} =
        Aperture.Teams.create("glados", %{
          "name" => team,
          "project" => repo,
          "base" => "HEAD",
          "members" => [
            %{
              "role" => "dev",
              "brief" => "Build it.",
              "harness" => "claude",
              "model" => "sonnet",
              "worktree" => true
            }
          ]
        })

      on_exit(fn -> Aperture.Teams.dissolve("glados", team) end)
      wait(fn -> Presence.state(member) != :offline end, "member never online")

      {:ok, view, _} = page()
      assert has_element?(view, "#team-#{team}")
      assert has_element?(view, "#team-#{team} [data-agent=#{member}]")

      # a row, not a card: role, harness · model, its branch
      row = view |> element("#member-#{member} .member-row") |> render()
      assert row =~ "dev"
      assert row =~ "claude · sonnet"
      assert row =~ "team/#{team}/dev"
      refute has_element?(view, "#agent-#{member}")

      # ⋯ opens the same controls as a coordinator's card
      refute has_element?(view, "#cfg-#{member}")
      view |> element("#member-#{member} .more") |> render_click()
      assert has_element?(view, "#cfg-#{member}")
      assert view |> element("#member-#{member} button.power") |> render_click() =~ "Confirm stop"
      send(view.pid, {:disarm, {:stop, member}})
      view |> element("#member-#{member} .more") |> render_click()
      refute has_element?(view, "#cfg-#{member}")

      dissolve = "#team-#{team} button[phx-click=dissolve]"
      assert view |> element(dissolve) |> render_click() =~ "Confirm dissolve"
      assert Aperture.Teams.get(team)

      view |> element(dissolve) |> render_click()
      wait(fn -> render(view) =~ "Team #{team} dissolved" end, "no dissolved toast")
      refute has_element?(view, "#team-#{team}")
      assert Aperture.Teams.get(team) == nil
      assert Presence.state(member) == :offline
    end
  end
end
