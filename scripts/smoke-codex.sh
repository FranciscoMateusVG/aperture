#!/usr/bin/env bash
# Real end-to-end smoke: GLaDOS (Claude Code) <-> Wheatley (Codex) through the v4 hub.
#
#   tmux window "hub"          -> the Elixir hub (mix run --no-halt)
#   tmux window "wheatley-srv" -> codex app-server --listen unix://<sock>
#   tmux window "wheatley"     -> codex resume <thread> --remote unix://<sock>  (your pane)
#   tmux window "glados"       -> Claude Code, inbox via Monitor, tools via aperture-mcp
#
# The hub's Codex bridge connects to the app-server socket, creates the thread,
# sends the kickoff, and delivers messages as turns. The pane only views it.
#
# Pass: the glados window shows "[aperture] message #N from wheatley: pong ..."
#
# Usage: bash scripts/smoke-codex.sh          (then: tmux attach -t aperture-v4)
#        bash scripts/smoke-codex.sh --kill
set -euo pipefail

SESSION=aperture-v4
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN_DIR="$ROOT/data/run"            # must match :run_dir in config/config.exs
HUB_HTTP="http://127.0.0.1:4518"
CODEX_AGENT=wheatley
SOCK="$RUN_DIR/$CODEX_AGENT.sock"
# Both the server and its viewing pane use the operator's normal login.
unset CODEX_HOME

if [[ "${1:-}" == "--kill" ]]; then
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  rm -f "$SOCK"
  echo "killed $SESSION"
  exit 0
fi

for bin in tmux claude codex curl; do
  command -v "$bin" >/dev/null || { echo "$bin not found"; exit 1; }
done
node -e 'process.exit(Number(process.versions.node.split(".")[0]) >= 22 ? 0 : 1)' \
  || { echo "need Node 22+ (built-in WebSocket)"; exit 1; }
tmux has-session -t "$SESSION" 2>/dev/null && { echo "$SESSION already running (use --kill)"; exit 1; }

mkdir -p "$RUN_DIR"
chmod 700 "$RUN_DIR"
rm -f "$SOCK"

# Resolve the effective MCP list, including plugin-provided servers. Only
# names and launch commands are used; credentials and user config stay put.
# Codex 0.158 accepts mcp_servers.<name>.enabled=false. Plugin-provided
# entries also need a transport when materialized as command-line config.
# Write shell-quoted arguments to a launcher so tmux never parses TOML.
launcher="$RUN_DIR/$CODEX_AGENT-launch.sh"
node --input-type=module - "$ROOT" "$CODEX_AGENT" "$HUB_HTTP" "$SOCK" "$launcher" <<'JS'
import { execFileSync } from 'node:child_process';
import { writeFileSync, chmodSync } from 'node:fs';

const [root, agent, hub, socket, launcher] = process.argv.slice(2);
const list = (overrides = []) => JSON.parse(execFileSync(
  'codex', ['mcp', 'list', '--json', ...overrides], { encoding: 'utf8' }
));
const overrides = ['-c', 'check_for_update_on_startup=false', '-c', 'approval_policy="never"', '-c', 'sandbox_mode="danger-full-access"'];
for (const server of list()) {
  if (server.name === 'aperture') continue;
  if (!/^[A-Za-z0-9_-]+$/.test(server.name)) {
    throw new Error(`Unsupported MCP override name: ${server.name}`);
  }
  const key = `mcp_servers.${server.name}`;
  if (server.transport.type === 'stdio') {
    overrides.push('-c', `${key}.command=${JSON.stringify(server.transport.command)}`);
  }
  overrides.push('-c', `${key}.enabled=false`);
}
overrides.push('-c', 'mcp_servers.aperture={enabled=true,command="node",args=' +
  JSON.stringify([`${root}/bin/aperture-mcp.mjs`, '--agent', agent, '--hub', hub]) + '}');
// Approve only Aperture's two tools.
overrides.push('-c', 'mcp_servers.aperture.tools.send_message.approval_mode="approve"',
  '-c', 'mcp_servers.aperture.tools.list_agents.approval_mode="approve"');

const enabled = list(overrides).filter(server => server.enabled).map(server => server.name);
if (enabled.length !== 1 || enabled[0] !== 'aperture') {
  throw new Error(`Expected only aperture MCP enabled, got: ${enabled.join(', ')}`);
}
console.log('Codex MCP preflight: only aperture enabled');

const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
const flags = overrides.map(quote).join(' ');
// Remote viewers inherit the server's permissions and MCP configuration.
const paneFlags = ['-c', 'check_for_update_on_startup=false'].map(quote).join(' ');
writeFileSync(launcher, `#!/usr/bin/env bash
set -euo pipefail
unset CODEX_HOME
cd ${quote(root)}
case "\${1:-server}" in
  server)
    exec codex app-server --listen ${quote(`unix://${socket}`)} ${flags}
    ;;
  pane)
    IFS= read -r thread_id < ${quote(`${root}/data/run/${agent}.thread-id`)}
    [[ "$thread_id" =~ ^[A-Za-z0-9-]{1,128}$ ]] || { echo "bad thread id"; exit 1; }
    exec codex resume "$thread_id" --remote ${quote(`unix://${socket}`)} ${paneFlags}
    ;;
  *) echo "usage: $0 [server|pane]" >&2; exit 2 ;;
esac
`, { mode: 0o700 });
chmodSync(launcher, 0o700);
JS

wait_for() {  # wait_for <seconds> <description> <test command...>
  local secs="$1" what="$2"; shift 2
  echo -n "waiting for $what"
  for _ in $(seq 1 "$secs"); do "$@" && { echo " ok"; return 0; }; echo -n "."; sleep 1; done
  echo " timed out"; return 1
}

# 1. Hub
tmux new-session -d -s "$SESSION" -n hub -c "$ROOT" "mix run --no-halt; read"
wait_for 60 "hub" curl -sf "$HUB_HTTP/health" -o /dev/null

# 2. Codex app-server for wheatley, then the bridge
printf -v server_command 'exec bash %q server' "$launcher"
tmux new-window -t "$SESSION" -n "$CODEX_AGENT-srv" -c "$ROOT" \
  "$server_command"
wait_for 30 "codex app-server socket" test -S "$SOCK"

curl -sf -X POST "$HUB_HTTP/codex/bridges" -H 'content-type: application/json' \
  -d "{\"agent\":\"$CODEX_AGENT\",\"socket\":\"$SOCK\"}" >/dev/null
bridge_bound() {
  curl -sf "$HUB_HTTP/agents" | node --input-type=module -e '
    let input = "";
    for await (const chunk of process.stdin) input += chunk;
    process.exit(JSON.parse(input).agents.includes(process.argv[1]) ? 0 : 1);
  ' "$CODEX_AGENT"
}
# The owned-thread file survives restarts, so its existence is not readiness.
wait_for 30 "bridge to register $CODEX_AGENT" bridge_bound
THREAD_ID="$(cat "$RUN_DIR/$CODEX_AGENT.thread-id")"
[[ "$THREAD_ID" =~ ^[A-Za-z0-9-]{1,128}$ ]] || { echo "bad thread id"; exit 1; }

# 3. Your view of wheatley: the TUI attached to the bridge's thread
printf -v pane_command 'bash %q pane; read' "$launcher"
tmux new-window -t "$SESSION" -n "$CODEX_AGENT" -c "$ROOT" \
  "$pane_command"

# 4. GLaDOS (Claude Code)
mcp="$RUN_DIR/glados.mcp.json"
cat > "$mcp" <<JSON
{"mcpServers":{"aperture":{"command":"node","args":["$ROOT/bin/aperture-mcp.mjs","--agent","glados","--hub","$HUB_HTTP"]}}}
JSON
rules="You are glados, an Aperture agent. Messages from other agents arrive as lines starting with [aperture] from your inbox monitor. Reply and send with the aperture send_message tool. Never type into tmux."
kickoff="Start your inbox now: use the Monitor tool to run \`node $ROOT/bin/aperture-inbox.mjs --agent glados --url ws://127.0.0.1:4518/inbox\` and keep it running for the whole session. Then send $CODEX_AGENT the message 'ping from glados, reply with pong' using send_message, wait for the reply and tell me when it arrives."
tmux new-window -t "$SESSION" -n glados -c "$ROOT" \
  claude --mcp-config "$mcp" --append-system-prompt "$rules" "$kickoff"

echo
echo "Started. Attach with:  tmux attach -t $SESSION"
echo "Pass: the glados window shows  [aperture] message #N from $CODEX_AGENT: pong ..."
echo "Connected agents:  curl -s $HUB_HTTP/agents"
