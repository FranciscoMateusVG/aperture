defmodule Aperture.TeamsTest do
  @moduledoc """
  Teams end to end through the hub's HTTP API (what GLaDOS's MCP tools call):
  real git repositories and worktrees, real tmux, fake `claude`/`codex`
  binaries (fake-claude runs the real inbox; the test plays Codex's
  app-server), and the real MCP server for the lead's tools.
  """
  use ExUnit.Case, async: false

  alias Aperture.{AgentFixtures, Codex, FakeCodex, NodeClient, Tmux}

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

  # -- helpers ------------------------------------------------------------------

  defp port, do: Application.fetch_env!(:aperture, :port)
  defp session, do: Application.fetch_env!(:aperture, :tmux_session)
  defp log_dir, do: Application.fetch_env!(:aperture, :launch_env)["FAKE_LOG_DIR"]
  defp worktrees, do: Application.fetch_env!(:aperture, :worktrees_dir)
  defp wait(fun, msg, timeout \\ 10_000), do: Aperture.FakeAgent.wait_until(fun, msg, timeout)

  defp request(method, path, body \\ nil) do
    {:ok, conn} = Mint.HTTP.connect(:http, "127.0.0.1", port(), mode: :passive)
    hdrs = [{"content-type", "application/json"}]
    {:ok, conn, ref} = Mint.HTTP.request(conn, method, path, hdrs, body && JSON.encode!(body))
    {status, data} = recv(conn, ref, nil, "")
    Mint.HTTP.close(conn)
    {status, JSON.decode!(data)}
  end

  defp recv(conn, ref, status, acc) do
    {:ok, conn, responses} = Mint.HTTP.recv(conn, 0, 30_000)

    {status, acc, done} =
      Enum.reduce(responses, {status, acc, false}, fn
        {:status, ^ref, s}, {_, a, d} -> {s, a, d}
        {:data, ^ref, d}, {s, a, done} -> {s, a <> d, done}
        {:done, ^ref}, {s, a, _} -> {s, a, true}
        _, x -> x
      end)

    if done, do: {status, acc}, else: recv(conn, ref, status, acc)
  end

  defp file(name) do
    path = Path.join(log_dir(), name)
    wait(fn -> match?({:ok, %{size: n}} when n > 0, File.stat(path)) end, "#{name} never written")
    File.read!(path)
  end

  defp args(name), do: name |> file() |> String.split(<<0>>, trim: true)
  defp cwd(name), do: name |> file() |> String.trim()
  defp value_after(args, flag), do: Enum.at(args, Enum.find_index(args, &(&1 == flag)) + 1)

  defp git(dir, args) do
    {out, 0} = System.cmd("git", ["-C", dir | args], stderr_to_stdout: true)
    String.trim(out)
  end

  defp branch?(repo, branch),
    do:
      match?(
        {_, 0},
        System.cmd("git", [
          "-C",
          repo,
          "rev-parse",
          "--verify",
          "--quiet",
          "refs/heads/" <> branch
        ])
      )

  # a throwaway repository with one commit
  defp git_repo do
    dir = Path.join(System.tmp_dir!(), "aperture-test-repo-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    git(dir, ["init", "-q"])
    File.write!(Path.join(dir, "README.md"), "hello\n")
    git(dir, ["add", "."])
    git(dir, ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"])
    on_exit(fn -> File.rm_rf(dir) end)
    dir
  end

  defp team_name, do: "t#{System.unique_integer([:positive])}"

  defp member(role, attrs \\ %{}) do
    Map.merge(
      %{role: role, brief: "Do the #{role} work.", harness: "claude", model: "sonnet"},
      attrs
    )
  end

  defp create(name, project, members, from \\ "glados") do
    request("POST", "/teams", %{from: from, name: name, project: project, members: members})
  end

  defp connected?(name), do: name in Aperture.Router.connected_agents()

  # -- tests --------------------------------------------------------------------

  test "GLaDOS creates a team: members launch on the harness and model she chose, coders in their own worktree" do
    repo = git_repo()
    team = team_name()
    coder = "#{team}-coder"
    reviewer = "#{team}-reviewer"

    members = [
      member("coder", %{brief: "Add the login form.\nReuse the existing styles.", worktree: true}),
      member("reviewer", %{
        brief: "Review the coder's branch.",
        harness: "codex",
        model: "gpt-6-astra"
      })
    ]

    # Claude's own config, with things in it that must survive
    claude_json = Application.fetch_env!(:aperture, :claude_json)

    File.write!(
      claude_json,
      JSON.encode!(%{
        "numStartups" => 7,
        "projects" => %{"/elsewhere" => %{"allowedTools" => ["x"]}}
      })
    )

    # the Codex member waits for its app-server: play it once fake-codex is up
    task = Task.async(fn -> create(team, repo, members) end)
    "unix://" <> socket = value_after(args("codex-app-server-#{reviewer}.args"), "--listen")
    _fake = FakeCodex.start(socket)
    assert {200, %{"result" => result}} = Task.await(task, 30_000)

    by_role = Map.new(result["members"], &{&1["role"], &1})
    assert %{"status" => "started", "harness" => "claude", "model" => "sonnet"} = by_role["coder"]

    assert %{"status" => "started", "harness" => "codex", "model" => "gpt-6-astra"} =
             by_role["reviewer"]

    # the coder: own worktree on its own branch, launched there, unattended
    worktree = Path.join(worktrees(), coder)
    assert by_role["coder"]["cwd"] == worktree
    assert by_role["coder"]["branch"] == "team/#{team}/coder"
    assert git(worktree, ["branch", "--show-current"]) == "team/#{team}/coder"
    assert cwd("claude-#{coder}.cwd") == worktree

    # its folder was trusted in Claude's config before launch (no trust prompt
    # with nobody at the keyboard); nothing else in that file changed
    claude = claude_json |> File.read!() |> JSON.decode!()
    assert claude["projects"][worktree] == %{"hasTrustDialogAccepted" => true}
    assert claude["projects"]["/elsewhere"] == %{"allowedTools" => ["x"]}
    assert claude["numStartups"] == 7
    refute Map.has_key?(claude["projects"], repo), "only Claude members' folders"

    claude = args("claude-#{coder}.args")
    assert value_after(claude, "--model") == "sonnet"
    assert "--dangerously-skip-permissions" in claude

    prompt = value_after(claude, "--append-system-prompt")
    assert prompt =~ "Add the login form.\nReuse the existing styles."
    assert prompt =~ "the **coder** of team **#{team}**"
    assert prompt =~ "`#{reviewer}`: reviewer, on codex / gpt-6-astra"
    assert prompt =~ "branch `team/#{team}/coder`"
    assert prompt =~ "You cannot message the operator"
    refute prompt =~ "{{"

    # the reviewer: Codex, in the project itself
    assert cwd("codex-app-server-#{reviewer}.cwd") == repo
    server = args("codex-app-server-#{reviewer}.args")
    assert ["--dangerously-bypass-approvals-and-sandbox", "app-server" | _] = server
    refute "--dangerously-bypass-approvals-and-sandbox" in args("codex-resume-#{reviewer}.args")
    assert "model=\"gpt-6-astra\"" in server

    instructions =
      server
      |> Enum.find(&String.starts_with?(&1, "model_instructions_file="))
      |> String.split("=", parts: 2)
      |> List.last()
      |> JSON.decode!()

    assert File.read!(instructions) =~ "The project directory `#{repo}`"

    wait(fn -> connected?(coder) and connected?(reviewer) end, "members never came online")
    assert coder in Tmux.windows(session())

    # the launcher page shows the team with its members
    {200, state} = request("GET", "/ui/state")

    assert %{"members" => [^coder, ^reviewer], "lead" => "glados", "project" => ^repo} =
             Enum.find(state["teams"], &(&1["name"] == team))

    assert %{"team" => ^team} = Enum.find(state["agents"], &(&1["name"] == coder))
    assert {200, _} = request("POST", "/teams/#{team}/dissolve", %{from: "glados"})
  end

  test "only a lead creates teams, and bad requests change nothing" do
    repo = git_repo()

    plain =
      Path.join(System.tmp_dir!(), "aperture-test-plain-#{System.unique_integer([:positive])}")

    File.mkdir_p!(plain)
    team = team_name()
    taken = AgentFixtures.define("#{team}-coder")

    refused = [
      {403, create(team, repo, [member("coder")], "wheatley")},
      {422, create(team, repo, [member("coder")])},
      {422, create(team, repo, [member("dev"), member("dev")])},
      {422, create(team, repo, [member("dev", %{harness: "cursor"})])},
      {422, create(team, repo, [member("dev", %{model: nil})])},
      {422, create(team, repo, [member("dev", %{brief: " "})])},
      {422, create(team, "relative/path", [member("dev")])},
      {422, create(team, plain, [member("dev", %{worktree: true})])},
      {422, create("Bad Name", repo, [member("dev")])},
      {422, create(team, repo, for(i <- 1..7, do: member("m#{i}")))}
    ]

    for {expected, {status, body}} <- refused do
      assert status == expected, inspect(body)
      assert is_binary(body["error"])
    end

    {_, %{"error" => taken_error}} = create(team, repo, [member("coder")])
    assert taken_error =~ "#{taken} already exists"
    {_, %{"error" => model_error}} = create(team, repo, [member("dev", %{model: nil})])
    assert model_error =~ "choose a model"

    {200, %{"teams" => teams}} = request("GET", "/teams")
    refute Enum.any?(teams, &(&1["name"] == team))
    refute File.exists?(Path.join(worktrees(), "#{team}-dev"))
    refute branch?(repo, "team/#{team}/dev")
  end

  test "members report to their lead: a message to the operator is refused" do
    team = team_name()
    member = "#{team}-dev"
    assert {200, _} = create(team, git_repo(), [member("dev")])

    assert {403, %{"error" => error}} =
             request("POST", "/messages", %{from: member, to: "operator", body: "hi boss"})

    assert error =~ "report to glados"

    assert {200, %{"status" => _}} =
             request("POST", "/messages", %{from: member, to: "glados", body: "done"})

    # standing agents still reach the operator
    assert {200, _} = request("POST", "/messages", %{from: "peppy", to: "operator", body: "ok?"})
    assert {200, _} = request("POST", "/teams/#{team}/dissolve", %{from: "glados"})
  end

  test "dissolving stops the members, removes clean worktrees, keeps dirty ones and every branch" do
    repo = git_repo()
    team = team_name()
    [a, b] = ["#{team}-coder", "#{team}-fixer"]

    assert {200, _} =
             create(team, repo, [
               member("coder", %{worktree: true}),
               member("fixer", %{worktree: true})
             ])

    wait(fn -> connected?(a) and connected?(b) end, "members never came online")
    [wt_a, wt_b] = Enum.map([a, b], &Path.join(worktrees(), &1))

    # the coder committed its work; the fixer left something uncommitted
    File.write!(Path.join(wt_a, "login.ex"), "form\n")
    git(wt_a, ["add", "."])
    git(wt_a, ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "login form"])
    File.write!(Path.join(wt_b, "wip.ex"), "half done\n")

    assert {403, _} = request("POST", "/teams/#{team}/dissolve", %{from: "wheatley"})

    assert {200, %{"result" => result}} =
             request("POST", "/teams/#{team}/dissolve", %{from: "glados"})

    by_name = Map.new(result["members"], &{&1["name"], &1})
    assert by_name[a]["worktree"] == "removed"
    assert by_name[b]["worktree"] == "kept (uncommitted changes): #{wt_b}"
    refute File.exists?(wt_a)
    assert File.read!(Path.join(wt_b, "wip.ex")) == "half done\n"
    assert branch?(repo, "team/#{team}/coder") and branch?(repo, "team/#{team}/fixer")
    assert git(repo, ["log", "-1", "--format=%s", "team/#{team}/coder"]) == "login form"

    refute a in Tmux.windows(session())
    wait(fn -> not connected?(a) and not connected?(b) end, "members still online")
    assert Aperture.Agents.get(a) == nil
    {200, %{"teams" => teams}} = request("GET", "/teams")
    refute Enum.any?(teams, &(&1["name"] == team))
    assert {404, _} = request("POST", "/teams/#{team}/dissolve", %{from: "glados"})

    # the name can be used again; the kept branch is not reused
    assert {200, %{"result" => %{"members" => [again]}}} =
             create(team, repo, [member("coder", %{worktree: true})])

    assert again["branch"] == "team/#{team}/coder-2"
    assert {200, _} = request("POST", "/teams/#{team}/dissolve", %{from: "operator"})
  end

  test "the lead's MCP server has the team tools; nobody else's does" do
    http = "http://127.0.0.1:#{port()}"
    repo = git_repo()
    team = team_name()

    tools = fn agent, extra ->
      mcp = NodeClient.start("aperture-mcp.mjs", ["--agent", agent, "--hub", http | extra])
      on_exit(fn -> NodeClient.kill(mcp) end)

      rpc = fn id, method, params ->
        NodeClient.write(
          mcp,
          JSON.encode!(%{jsonrpc: "2.0", id: id, method: method, params: params}) <> "\n"
        )

        mcp |> NodeClient.wait_line(&String.contains?(&1, ~s("id":#{id}))) |> JSON.decode!()
      end

      rpc.(1, "initialize", %{
        protocolVersion: "2025-06-18",
        capabilities: %{},
        clientInfo: %{name: "t", version: "0"}
      })

      %{"result" => %{"tools" => list}} = rpc.(2, "tools/list", %{})
      {Enum.map(list, & &1["name"]), rpc}
    end

    {names, _} = tools.("wheatley", [])
    refute "create_team" in names

    {names, rpc} = tools.("glados", ["--lead", "true"])
    assert ~w(create_team dissolve_team list_teams) -- names == []

    assert %{"result" => %{"content" => [%{"text" => text}]}} =
             rpc.(3, "tools/call", %{
               name: "create_team",
               arguments: %{name: team, project: repo, members: [member("dev")]}
             })

    assert %{"team" => ^team, "members" => [%{"name" => dev, "status" => "started"}]} =
             JSON.decode!(text)

    assert %{"result" => %{"content" => [%{"text" => listed}]}} =
             rpc.(4, "tools/call", %{name: "list_teams", arguments: %{}})

    assert listed =~ dev

    assert %{"result" => %{"content" => [%{"text" => gone}]}} =
             rpc.(5, "tools/call", %{name: "dissolve_team", arguments: %{name: team}})

    assert %{"dissolved" => true} = JSON.decode!(gone)
    refute File.exists?(Codex.thread_id_path(dev))
  end
end
