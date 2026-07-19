#!/usr/bin/env bash
# smoke-boot.sh — L2/L3 boot verification orchestrator (aperture-xt16e).
#
# Boots the WS hub, watches it with observer.mjs, spawns one claude agent and
# one codex agent (stubs in MODE=l2, real binaries in MODE=l3), and asserts
# both presence-join within the deadline. On failure, collects artifacts
# (hub log, tmux pane captures, stub argv logs) into the run dir.
#
# Env:
#   MODE=l2|l3            l2 = stub CLIs (default), l3 = real harnesses
#   TIMEOUT_S=<n>         observer deadline (default 60)
#   APERTURE_BOOT_CMD=…   escape hatch: eval'd to perform the agent spawn
#                         while the headless boot entry point is blocked on
#                         aperture-syepg (see SPAWN SECTION below)
#   APERTURE_BOOT_RUN_DIR run-dir root (default /tmp/aperture-boot-harness)
#
# Exit codes: 0 = green, 1 = boot verification failed, 3 = BLOCKED on
# aperture-syepg (no entry point and no APERTURE_BOOT_CMD).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
MODE="${MODE:-l2}"
TIMEOUT_S="${TIMEOUT_S:-60}"
HUB_JS="$REPO/mcp-server/dist/ws-hub.js"

if [ "$MODE" != "l2" ] && [ "$MODE" != "l3" ]; then
    echo "❌ MODE must be l2 or l3 (got: $MODE)"
    exit 2
fi
if [ ! -f "$HUB_JS" ]; then
    echo "❌ $HUB_JS missing — run: just build-mcp"
    exit 2
fi
if [ ! -d "$REPO/mcp-server/node_modules/ws" ]; then
    echo "❌ mcp-server/node_modules/ws missing — run: cd mcp-server && pnpm install"
    exit 2
fi

# ── (a) run dir + free port ──
RUN_ROOT="${APERTURE_BOOT_RUN_DIR:-/tmp/aperture-boot-harness}"
RUN_DIR="$RUN_ROOT/run-$(date +%Y%m%d-%H%M%S)-$$"
mkdir -p "$RUN_DIR"
export APERTURE_STUB_LOG_DIR="$RUN_DIR/stubs"
mkdir -p "$APERTURE_STUB_LOG_DIR"

PORT="$(node -e 'const s=require("net").createServer();s.listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})')"
export APERTURE_WS_PORT="$PORT"
echo "🔍 smoke-boot MODE=$MODE port=$PORT run-dir=$RUN_DIR"

HUB_PID=""
OBS_PID=""
cleanup() {
    # Kill any backgrounded spawn-section children first, then observer + hub.
    local pids
    pids="$(jobs -p || true)"
    [ -n "${OBS_PID}" ] && kill "$OBS_PID" 2>/dev/null || true
    [ -n "${HUB_PID}" ] && kill "$HUB_PID" 2>/dev/null || true
    if [ -n "$pids" ]; then
        # shellcheck disable=SC2086
        kill $pids 2>/dev/null || true
    fi
    wait 2>/dev/null || true
}
trap cleanup EXIT

# ── (b) start hub, wait for the listening event ──
APERTURE_WS_PORT="$PORT" APERTURE_HUB_SKIP_REPLAY=1 \
    node "$HUB_JS" 2> "$RUN_DIR/hub.log" &
HUB_PID=$!

listening=0
for _ in $(seq 1 40); do
    if grep -q '"event":"listening"' "$RUN_DIR/hub.log" 2>/dev/null; then
        listening=1
        break
    fi
    if ! kill -0 "$HUB_PID" 2>/dev/null; then
        break
    fi
    sleep 0.25
done
if [ "$listening" != "1" ]; then
    echo "❌ hub never logged 'listening' within 10s — see $RUN_DIR/hub.log"
    exit 1
fi
echo "✅ hub listening on 127.0.0.1:$PORT"

# ── (c) start the observer ──
node "$HERE/observer.mjs" \
    --port "$PORT" \
    --expect-agent claude-smoke \
    --expect-agent codex-smoke \
    --timeout-s "$TIMEOUT_S" \
    --hub-log "$RUN_DIR/hub.log" \
    --out "$RUN_DIR/results.json" \
    > "$RUN_DIR/observer.log" 2>&1 &
OBS_PID=$!

# Gate the spawn on the observer actually being subscribed — an agent hello
# that lands before the subscriber registers would broadcast join to nobody
# and the run would falsely time out (observed as a 6ms race in self-test).
subscribed=0
for _ in $(seq 1 40); do
    if grep -q '"event":"observer_subscribed"' "$RUN_DIR/observer.log" 2>/dev/null; then
        subscribed=1
        break
    fi
    if ! kill -0 "$OBS_PID" 2>/dev/null; then
        break
    fi
    sleep 0.25
done
if [ "$subscribed" != "1" ]; then
    echo "❌ observer never subscribed within 10s — see $RUN_DIR/observer.log"
    exit 1
fi
echo "🔍 observer watching for claude-smoke + codex-smoke (timeout ${TIMEOUT_S}s)"

# ── (d) SPAWN SECTION ──────────────────────────────────────────────────────
# TODO(aperture-syepg): headless boot entry point not yet available. When
# Peppy's entry point lands, invoke it HERE for one claude agent
# (claude-smoke) and one codex agent (codex-smoke), with the launcher-stub
# env knobs (being added by a parallel worker) selecting the binaries:
#
#   MODE=l2 → export APERTURE_CLAUDE_BIN="$HERE/stubs/claude"
#             export APERTURE_CODEX_BIN="$HERE/stubs/codex"
#             (or APERTURE_LAUNCHER_PATH_PREFIX="$HERE/stubs")
#   MODE=l3 → leave the knobs unset; real claude/codex binaries boot.
#
# Expected shape (adjust to the real entry-point CLI when it exists):
#   <headless-boot> --agent claude-smoke --agent codex-smoke
# ---------------------------------------------------------------------------
if [ "$MODE" = "l2" ]; then
    export APERTURE_CLAUDE_BIN="$HERE/stubs/claude"
    export APERTURE_CODEX_BIN="$HERE/stubs/codex"
    export APERTURE_LAUNCHER_PATH_PREFIX="$HERE/stubs"
fi

if [ -n "${APERTURE_BOOT_CMD:-}" ]; then
    # Escape hatch: lets us wire Peppy's entry point (or launch stubs
    # directly) without editing this script's structure.
    echo "🔍 spawning agents via APERTURE_BOOT_CMD"
    eval "$APERTURE_BOOT_CMD"
else
    echo "❌ BLOCKED: headless entry point (aperture-syepg)"
    echo "   No agent spawn is wired yet. Provide APERTURE_BOOT_CMD as an"
    echo "   escape hatch, or wait for the aperture-syepg entry point."
    exit 3
fi

# ── (e) wait for the observer verdict ──
set +e
wait "$OBS_PID"
OBS_EXIT=$?
set -e
OBS_PID=""

if [ "$OBS_EXIT" = "0" ]; then
    echo "✅ boot verification green (MODE=$MODE)"
    echo ""
    echo "── time-to-hello (ms) ──"
    node -e '
        const r = require(process.argv[1]);
        for (const [agent, ms] of Object.entries(r.time_to_hello_ms)) {
            console.log(`  ${agent.padEnd(20)} ${ms} ms`);
        }
        const binds = Object.entries(r.time_to_bind_ms ?? {});
        for (const [agent, ms] of binds) {
            console.log(`  ${agent.padEnd(20)} ${ms} ms (codex_bound)`);
        }
    ' "$RUN_DIR/results.json"
    echo ""
    echo "   results: $RUN_DIR/results.json"
    exit 0
fi

# ── failure: collect artifacts ──
echo "⚠️  observer exited $OBS_EXIT — collecting artifacts"
if command -v tmux > /dev/null 2>&1 && tmux has-session -t aperture 2>/dev/null; then
    mkdir -p "$RUN_DIR/tmux"
    while IFS= read -r pane; do
        safe="$(echo "$pane" | tr ':.%' '___')"
        tmux capture-pane -p -t "aperture:$pane" > "$RUN_DIR/tmux/pane-$safe.txt" 2>/dev/null || true
    done < <(tmux list-panes -s -t aperture -F '#{window_index}.#{pane_index}' 2>/dev/null)
    echo "   tmux pane captures → $RUN_DIR/tmux/"
fi
# hub.log, observer.log, results.json, stubs/*-argv.txt are already in RUN_DIR.
echo "❌ boot verification FAILED (MODE=$MODE) — artifacts: $RUN_DIR"
echo "   hub.log observer.log results.json stubs/ $( [ -d "$RUN_DIR/tmux" ] && echo tmux/ )"
exit 1
