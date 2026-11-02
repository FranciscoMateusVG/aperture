#!/usr/bin/env bash
# Real end-to-end smoke: two real Claude Code agents talk through the v4 hub.
#
#   tmux window "hub"      -> the Elixir hub (mix run --no-halt)
#   tmux window "glados"   -> Claude Code, inbox via Monitor, tools via aperture-mcp
#   tmux window "wheatley" -> same
#
# Pass: GLaDOS's pane shows "[aperture] message #N from wheatley: pong ..."
# Approve the Monitor / MCP tool prompts in each pane the first time.
#
# Usage: scripts/smoke-claude.sh            (then: tmux attach -t aperture-v4)
#        scripts/smoke-claude.sh --kill     (tear down)
set -euo pipefail

SESSION=aperture-v4
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN_DIR="${TMPDIR:-/tmp}/aperture-v4"
HUB_HTTP="http://127.0.0.1:4518"

if [[ "${1:-}" == "--kill" ]]; then
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  echo "killed $SESSION"
  exit 0
fi

command -v tmux >/dev/null || { echo "tmux not found"; exit 1; }
command -v claude >/dev/null || { echo "claude CLI not found"; exit 1; }
node -e 'process.exit(Number(process.versions.node.split(".")[0]) >= 22 ? 0 : 1)' \
  || { echo "need Node 22+ (built-in WebSocket)"; exit 1; }

mkdir -p "$RUN_DIR"
tmux has-session -t "$SESSION" 2>/dev/null && { echo "$SESSION already running (use --kill)"; exit 1; }

# 1. Hub
tmux new-session -d -s "$SESSION" -n hub -c "$ROOT" "mix run --no-halt; read"
echo -n "waiting for hub"
for _ in $(seq 1 60); do
  curl -sf "$HUB_HTTP/health" >/dev/null && break
  echo -n "."; sleep 1
done
curl -sf "$HUB_HTTP/health" >/dev/null || { echo " hub did not start (see tmux window 'hub')"; exit 1; }
echo " up"

launch_agent() {
  local name="$1" task="$2"
  local mcp="$RUN_DIR/$name.mcp.json"
  cat > "$mcp" <<JSON
{"mcpServers":{"aperture":{"command":"node","args":["$ROOT/bin/aperture-mcp.mjs","--agent","$name","--hub","$HUB_HTTP"]}}}
JSON

  local rules="You are $name, an Aperture agent. Messages from other agents arrive as lines starting with [aperture] from your inbox monitor. Reply and send with the aperture send_message tool. Never type into tmux."
  local kickoff="Start your inbox now: use the Monitor tool to run \`node $ROOT/bin/aperture-inbox.mjs --agent $name --url ws://127.0.0.1:4518/inbox\` and keep it running for the whole session. Then: $task"

  tmux new-window -t "$SESSION" -n "$name" -c "$ROOT" \
    claude --mcp-config "$mcp" --append-system-prompt "$rules" "$kickoff"
}

# 2. Agents. GLaDOS may send before Wheatley's inbox is up: the hub queues it.
launch_agent glados "send wheatley the message 'ping from glados, reply with pong' using send_message, then wait for the reply and tell me when it arrives."
launch_agent wheatley "wait for messages. When one arrives from glados asking for pong, reply to glados with 'pong from wheatley'."

echo
echo "Started. Attach with:  tmux attach -t $SESSION"
echo "Watch the glados window for:  [aperture] message #N from wheatley: pong from wheatley"
echo "Connected agents:  curl -s $HUB_HTTP/agents"
