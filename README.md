# Aperture v4

The message hub from the v4 spec, the Claude-side clients, the Codex bridge,
and the launcher for the three standing agents.
Messages are stored in SQLite before they are pushed, every push is acked,
and anything unacked is replayed in order when the agent reconnects.

## Run it

```bash
brew install elixir node   # Elixir 1.18+, Node 22+
mix deps.get
mix test                   # 53 integration tests, ~9 seconds (needs tmux + node)
bash scripts/up.sh         # hub + every agent in tmux session aperture-v4
tmux attach -t aperture-v4
bash scripts/down.sh       # stop everything (threads and choices are kept)
```

The hub listens on 127.0.0.1:4518 (v3 keeps 4517). Open **http://127.0.0.1:4518**
for the launcher page.

## Launcher page

One page, served by the hub, updated live (Server-Sent Events, no build step;
`priv/ui/index.html`):

- every agent with its state (offline / idle / busy), harness and model
- click an agent's name: your tmux session switches to its window
- change harness or model and **Apply** (saves and restarts that agent), or **Restart**
- message any agent as yourself (it arrives `from operator`)
- **Inbox**: what agents sent to `operator`, with a badge per agent; **Mark read** clears it

In the LiveView **Chat**, **Reset** opens a confirmation for the selected
conversation. **Confirm reset** archives its history; **Cancel**, closing the
chat, or selecting another agent cancels the confirmation. Nothing is deleted:
SQLite `chat_archives` records an append-only message-id boundary, and message
rows, attachments and delivery acknowledgements stay intact. Old history and
unread badges are hidden across open tabs and reloads; new messages work as
before. There is no archive browser yet; archived rows remain in `messages`.
Unsent drafts and pending deliveries are not cancelled by resetting history.

The optional **Also restart agent with fresh context** checkbox interrupts the
agent's current work. It calls `Launcher.restart_agent(name, fresh_context:
true)`: after stopping the bridge, the old Codex `.thread-id` pointer is renamed
to `.thread-id.archived-<unique suffix>` in the same run directory, without
removing any transcript, and a new thread starts. Claude launches without
`--resume`, `--continue` or a session id (as it already does on normal Restart).
Normal Codex **Restart** and **Apply** continue to resume the owned thread.
If restart fails, the chat reports it separately; the archive remains saved.

Only this machine can use it: the hub refuses requests whose Host isn't
loopback (DNS rebinding) or whose browser Origin isn't this page (other
websites posting to localhost).

## Agents

| Agent | Default harness | Default model | Prompt |
| --- | --- | --- | --- |
| GLaDOS | Claude Code | opus | `prompts/glados.md` |
| Peppy | Codex | gpt-6-astra | `prompts/peppy.md` |
| Wheatley | Codex | gpt-6-astra | `prompts/wheatley.md` |

Definitions live in `agents/<name>.json`; `prompts/_shared.md` is prepended to
every agent's prompt. A definition can give its agent extra MCP servers (`"mcp"`);
all three get v3's mempalace memory bank (`~/.aperture/mempalace`), so
credentials and lessons stay where they were. Peppy's v3 infrastructure
knowledge (the Known Infrastructure section and the `pocketsoftware-infra`,
`dokploy-api`, `deploy-workflow`, `wire-the-adapter` skills) is kept verbatim in
`knowledge/peppy/`, indexed from his prompt.

Harness and model are your choice per agent, saved in `data/agents.json`:

```bash
mix aperture.agent                                  # list
mix aperture.agent glados --model sonnet
mix aperture.agent wheatley --harness claude --model opus
```

If the hub is running, that agent restarts on the new harness/model right away
(`POST /agents/:name/config` does the same). Messages sent meanwhile wait in the
store; a Codex agent's thread is kept for when it goes back to Codex.

Each agent gets a tmux window named after it. Claude Code agents run
`claude --model … --mcp-config … --settings <busy/idle hooks> --append-system-prompt …`
and start `aperture-inbox` through the Monitor tool. Codex agents get a
`<name>-srv` window with `codex app-server` (model, prompt, `danger-full-access`,
`approval_policy=never`, the aperture MCP server, every other MCP server defined in
`~/.codex/config.toml` disabled) and a `<name>` pane
running `codex resume <thread> --remote`. All Codex agents share your `~/.codex`
login. Restarting only the hub reattaches running Codex agents without
relaunching them.

### Start, readiness and exit evidence

**Start** is not **Restart**. It checks the pane's OS child processes, not
just the existence of a tmux window. A retained wrapper with a recorded
CLI exit is recoverable; a live CLI or an uncertain/mid-start window is
never killed by Start. An old/untracked window whose liveness cannot be
established fails closed; use explicit Restart after inspecting it.
Codex's app-server is the agent process; a missing/exited viewer can be
restored without replacing a live server. An ambiguous half-pair is
preserved rather than automatically torn down.

A fresh launch reports **started** only after the inbox connects, with a
30-second default inbox deadline (`agent_start_timeout_ms`). An exit or
deadline produces **startup_failed**, visible in presence/the dashboard.
An unconnected CLI that times out is left running for inspection; Start
may wait again, while Restart remains the explicit destructive action.
The live inbox is transport readiness, not proof that every model/MCP
dependency is usable.

Private, per-run receipts under `data/run/lifecycle/<window>/<run-id>/`
record exit code and UTC time in `exit.json`, plus a separate startup
state. No transcript, argv, environment or credential content is logged.
Old receipts survive a subsequent launch. A force-killed wrapper (including
OS shutdown) cannot promise a final receipt; missing metadata is not proof
of exit. Startup of all agents at login remains unchanged.

## Teams

GLaDOS (any agent in `team_leads`) creates a team for one piece of work with
the `create_team` tool: a name, the project's absolute path and 1–6 members,
each with a role, a brief, a harness and a model (she always picks both).
Members are agents named `<team>-<role>`, launched in parallel, shown in their
own section on the launcher page and as tmux windows.

- `worktree: true` members (coders) get a git worktree under `data/worktrees/`
  on a new branch `team/<team>/<role>` (`-2`, `-3`… if taken); the rest work in
  the project directory. Claude members run with `--permission-mode
  bypassPermissions` (`team_claude_permission_mode`), Codex members with
  approval `never`, so nobody stalls on a prompt. Before a Claude member
  launches, its folder is marked trusted in `~/.claude.json`
  (`bin/aperture-trust.mjs`), so Claude's "trust this folder?" question
  doesn't stop it either. (Operator's decision: teams run unattended.)
- Their prompt is `prompts/_team.md` filled with the brief, the roster and the
  workspace. They report to the lead; a message from a member to `operator`
  is refused.
- `dissolve_team` (or Dissolve on the page) stops the members, removes clean
  worktrees, keeps any worktree with uncommitted or untracked work, and never
  deletes a branch.

Teams live in `data/teams.json`, so `scripts/up.sh` brings running teams back.

## Tasks (BEADS)

Agents get `create_task`, `list_tasks` (summaries; `ready: true` for open,
unblocked work), `show_task`, `claim_task` (in_progress + assigned to the
caller), `update_task` (status, assignee, appended notes, labels) and
`close_task` (with a reason). Only the hub runs `bd` (`POST /beads/:op`):
`BD_ACTOR` is the calling agent and the Dolt credentials stay in the hub's
environment, never in an agent's config. It uses v3's `~/.aperture/.beads`, so
existing tasks carry over. Free text is passed as `--flag=value`, and ids,
labels and names are validated before `bd` runs.

## Memory

Agents get `recall(query)` / `recall_full(key)`: read-only search over v3's
banked lessons and decisions (the BEADS memory bank as v3 indexed it,
`~/.aperture/run/memory-index.json`, already redacted by v3). Secret-flagged and
superseded memories are never returned, full texts are capped at 8 KB, and the
file is re-read when it changes. Nothing is written or migrated. Credentials stay
in mempalace, which every agent also gets.

`GET /presence` lists every agent with harness, model and `offline` / `idle` /
`busy`: Claude Code hooks report busy/idle via `bin/aperture-state.mjs`, Codex
agents via their bridge.

Run scripts as `bash scripts/...` and clients as `node bin/...`, so nothing
depends on execute bits surviving a zip.

## Real smoke tests

```bash
bash scripts/smoke-claude.sh   # glados + wheatley, both Claude Code
bash scripts/smoke-codex.sh    # glados (Claude Code) + wheatley (Codex)
tmux attach -t aperture-v4
bash scripts/smoke-codex.sh --kill
```

Approve the Monitor and MCP prompts in the `glados` pane. Pass = the `glados`
window shows `[aperture] message #N from wheatley: pong ...`.

The Codex smoke uses your normal `~/.codex` login for both Wheatley's app-server
and viewing pane. A generated `data/run/wheatley-launch.sh` passes per-process
overrides for `approval_policy = "never"`, `sandbox_mode = "workspace-write"`,
and Aperture's MCP tools. Other effective MCP servers are disabled with
`mcp_servers.<name>.enabled=false`; a preflight verifies only Aperture is enabled.
Your credentials and user configuration are not copied or changed.

`data/run/wheatley.thread-id` records Wheatley's owned thread and survives
`--kill` and restart. Startup waits for Wheatley in `/agents` before reading
that file and opening the pane. A new agent name starts with its own record.

## Layout

| File | What it does |
| --- | --- |
| `lib/aperture/application.ex` | Supervision tree: connection registry, store, HTTP endpoint |
| `lib/aperture/store.ex` | SQLite message log: insert, ack, unacked-in-order |
| `lib/aperture/router.ex` | Store first, then push if the recipient is connected |
| `lib/aperture/hub/endpoint.ex` | `/inbox` WebSocket, `POST /messages`, `GET /agents`, `/health` |
| `lib/aperture/hub/socket.ex` | One process per connected agent: hello, ack, send, ping |
| `lib/aperture/ws.ex` | Minimal WebSocket client on Mint (TCP or unix socket) |
| `lib/aperture/codex.ex` | Start/stop Codex bridges; thread-id handoff path |
| `lib/aperture/codex/bridge.ex` | One per Codex agent: app-server JSON-RPC, turns, acks, rebind |
| `lib/aperture/agents.ex` | Agent definitions + your saved harness/model choices |
| `lib/aperture/launcher.ex` | Starts/stops/switches agents in tmux per harness |
| `lib/aperture/presence.ex` | Busy/idle per agent |
| `lib/aperture/tmux.ex` | tmux CLI wrapper |
| `lib/aperture/beads.ex` | BEADS ops via `bd`, run only by the hub |
| `lib/aperture/teams.ex`, `lib/aperture/team_prompt.ex` | Teams: create (worktrees, parallel launch), dissolve, member prompt |
| `lib/aperture/recall.ex` | Read-only search over v3's banked memories |
| `lib/aperture/ui.ex`, `priv/ui/index.html` | Launcher page: snapshot, live stream, the page |
| `lib/mix/tasks/` | `mix aperture.up`, `mix aperture.agent` |
| `agents/`, `prompts/` | Agent definitions and prompts |
| `bin/aperture-state.mjs` | Claude Code hook: report busy/idle |
| `bin/aperture-inbox.mjs` | Receive side for Claude's Monitor: one stdout line per message; reconnects forever |
| `bin/aperture-mcp.mjs` | Stdio MCP server: messaging, BEADS tasks, recall; team tools for the lead |
| `scripts/up.sh`, `scripts/down.sh` | Start / stop everything |
| `scripts/smoke-*.sh` | M1 smoke scripts (superseded by `up.sh`) |
| `test/support/fake_agent.ex` | Scripted agent client speaking the real protocol |
| `test/support/node_client.ex` | Runs the real `bin/` scripts from tests |
| `test/support/fake_codex.ex` | Scripted `codex app-server` on a unix socket |
| `test/support/bin/fake-*` | Fake `claude` / `codex` / `bd` CLIs for tests |

Both `bin/` scripts are plain Node 22 with no dependencies.

## Protocol

WebSocket `/inbox`, JSON text frames:

| Frame | Direction | Fields |
| --- | --- | --- |
| `hello` | client → hub | `agent` |
| `welcome` | hub → client | `agent` (registered; replay follows) |
| `message` | hub → client | `id`, `from`, `body` |
| `ack` | client → hub | `id` |
| `send` | client → hub | `to`, `body`, optional `ref` |
| `sent` | hub → client | `id`, `status` (`pushed` / `queued`), `ref` |
| `ping` / `pong` | both | none |
| `error` | hub → client | `reason` |

HTTP: `POST /messages {"from","to","body"}` → `{"id","status"}`; `GET /agents` → `{"agents": [...]}`;
`POST /codex/bridges {"agent","socket"}` starts a Codex bridge; `DELETE /codex/bridges/:agent` stops it.

## Codex bridge

Per Codex agent, the hub connects to `codex app-server --listen unix://<socket>`:
`initialize` → resume the thread this agent owns (`data/run/<agent>.thread-id`,
kept across restarts), or, if it has none or it is gone, `thread/start` plus one
kickoff turn, recorded in that file. It never resumes a thread it did not create,
since Codex agents can share your `~/.codex` with your own sessions. Your pane
uses the same file for `codex resume <id> --remote unix://<socket>`. Messages go in as
`[aperture] message #N from X: ...` via `turn/steer` with `expectedTurnId`
when a turn is in progress, otherwise `turn/start`. The active turn id comes
from `turn/started`, the `turn/start` / `turn/steer` results, and the resumed
thread's `inProgress` turn; it is cleared when a turn completes or the thread
goes idle. A message is acked only when its inject call succeeds; a failed
steer forgets the turn id and retries as `turn/start` (codex 0.158 accepts that
mid-turn); anything unacked when the socket drops is re-injected after the
bridge rebinds. Shapes match the codex-cli 0.158 app-server schema. Approval requests from
the app-server are left to your pane.
The MCP shim sends over HTTP so it never competes with the agent's inbox socket.

## What the tests prove

Hub, with fake agents (`delivery_test.exs`):

1. A message from A to B arrives exactly once.
2. B offline when A sends → B gets it on connect.
3. B's socket drops mid-burst of 200 → all 200 arrive, in order, no duplicates.
4. Hub restart (endpoint and store) → clients reconnect, nothing queued is lost.
5. Acked messages are not redelivered after a reconnect.

Real clients against the real hub (`clients_test.exs`, skipped without Node):

6. `aperture-inbox` prints each message once, as one line (newlines escaped).
7. `aperture-inbox` survives a hub restart and keeps receiving.
8. `aperture-inbox` acks: a restarted inbox does not reprint old messages.
9. `aperture-mcp` `send_message` delivers; `list_agents` lists connected agents.
10. A newer inbox for the same agent replaces the old one, which exits (no flapping).

Codex bridge against a fake app-server (`codex_bridge_test.exs`):

11. Fresh app-server: one thread, one kickoff, thread id published.
12. Busy Codex gets `turn/steer` with the active turn id; idle Codex gets `turn/start`.
13. Resuming mid-turn: the bridge learns the in-progress turn and steers into it.
14. A missed turn notification: the stale steer falls back to `turn/start`.
15. Messages sent before Codex is up are injected once it binds.
16. Owned thread: resumed after a restart, no kickoff.
17. A thread it does not own (your own session) is never resumed.
18. An owned thread that is gone is replaced and the new id recorded.
19. Connection drop: same thread rebound, no second kickoff, acked messages not resent.
20. A failed steer is retried as `turn/start`.
21. Bridges start over HTTP; unsafe agent names are refused.

Agents, launcher and presence (`agents_test.exs`, `launcher_test.exs`, `presence_test.exs`):
saved choices override defaults; Claude agent launched with model, tools, hooks,
prompt and Monitor kickoff, then receives a message in its pane; Codex agent
launched with its overrides and personal MCP servers disabled, bridge bound, pane
resuming the thread; switching harness restarts on the other one and keeps the
thread; start is idempotent, stop takes the agent offline; a hub restart
reattaches a running Codex agent; offline/idle/busy for both harnesses.

Teams (`teams_test.exs`, real git and tmux): a mixed Claude/Codex team
launches on the chosen harnesses and models, the coder in its own worktree and
branch with `bypassPermissions`, the reviewer in the project, each with the
brief and roster in its prompt, and shows up on the page; only a lead may
create one and every bad request leaves no team, worktree or branch behind;
members cannot message the operator; dissolving stops everyone, removes the
clean worktree, keeps the dirty one and every branch (a reused name gets
`-2`); the lead's MCP server alone has the team tools, and they work end to end.

Each was checked by breaking the code on purpose (no replay, no ack, no
reconnect, no resume, no retry, always `turn/start`, steer without
`expectedTurnId`, stale turn ids kept) and confirming the matching tests fail.

## Not yet

- Re-push to a live but silent client (spec says 30s); today replay happens on reconnect.
- `from` on `POST /messages` is trusted; fine while the hub only listens on localhost.
