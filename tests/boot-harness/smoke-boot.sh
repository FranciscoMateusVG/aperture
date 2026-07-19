#!/usr/bin/env bash
# smoke-boot.sh — L2/L3 boot verification orchestrator (aperture-xt16e).
#
# Boots the WS hub, watches it with observer.mjs, then boots agents through
# the REAL spawn path: aperture-boot (headless entry, aperture-syepg) →
# agent_loader → boot_agent_process → tmux window → launcher script →
# claude/codex binaries (stubs in MODE=l2, real harnesses in MODE=l3).
# The harness itself never calls tmux send-keys — post-#37 boot is entirely
# CLI-arg driven (Claude kickoff positional / Codex resume gate); the only
# send-keys on the whole path is the product's own launcher-path injection
# inside boot_agent_process.
#
# Env:
#   MODE=l2|l3            l2 = stub CLIs (default), l3 = real harnesses
#   TIMEOUT_S=<n>         observer deadline (default 60)
#   FLEET=<n>             thundering-herd knob (failure mode #6): generate n
#                         extra claude agents (fleet-1..fleet-n) in a temp
#                         overlay registry and boot them all in one
#                         aperture-boot invocation
#   APERTURE_BOOT_CMD=…   explicit override: eval'd INSTEAD of aperture-boot
#                         (kept for direct-stub debugging)
#   APERTURE_BOOT_RUN_DIR run-dir root (default /tmp/aperture-boot-harness)
#
# Exit codes: 0 = green, 1 = boot verification failed, 2 = usage/setup error.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
MODE="${MODE:-l2}"
TIMEOUT_S="${TIMEOUT_S:-60}"
FLEET="${FLEET:-}"
HUB_JS="$REPO/mcp-server/dist/ws-hub.js"

# `node` on this machine can be a VOLTA SHIM that spawns the real node as a
# child and does NOT forward SIGTERM — killing the shim pid orphans the actual
# listener (observed: 15 leaked hubs). Resolve the real binary once and use it
# for every long-lived node process we must be able to kill by pid. Exported
# so stubs (which exec node in tmux panes) can use it too.
REAL_NODE="$(node -e 'console.log(process.execPath)')"
export APERTURE_REAL_NODE="$REAL_NODE"

if [ "$MODE" != "l2" ] && [ "$MODE" != "l3" ]; then
    echo "❌ MODE must be l2 or l3 (got: $MODE)"
    exit 2
fi

# Stale-dist guard: running against an outdated hub build produces
# deterministic false failures that mimic real bugs. Rebuild when dist is
# missing or older than any TS source.
dist_stale=0
if [ ! -f "$HUB_JS" ]; then
    dist_stale=1
else
    for src in "$REPO/mcp-server/src/"*.ts; do
        if [ "$src" -nt "$HUB_JS" ]; then
            dist_stale=1
            break
        fi
    done
fi
if [ "$dist_stale" = "1" ]; then
    echo "🔨 mcp-server dist missing/stale — rebuilding (cd mcp-server && pnpm build)"
    if ! (cd "$REPO/mcp-server" && pnpm build); then
        echo "❌ pnpm build failed in mcp-server"
        exit 2
    fi
fi
if [ ! -f "$HUB_JS" ]; then
    echo "❌ $HUB_JS still missing after build — run: just build-mcp"
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

PORT="$("$REAL_NODE" -e 'const s=require("net").createServer();s.listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})')"
export APERTURE_WS_PORT="$PORT"
echo "🔍 smoke-boot MODE=$MODE port=$PORT run-dir=$RUN_DIR${FLEET:+ fleet=$FLEET}"

# ── (a2) stub registry ──
# The real load path (agent_loader::load_agents_from_disk) wants, per agent:
#   <root>/<name>/manifest.json  (serde-required: name, model, window, role)
#   <root>/<name>/prompt.md
#   <root>/<name>/skills/<skill>/SKILL.md   (optional; exercised by inject_skills)
REGISTRY="$HERE/registry"
AGENTS=(claude-smoke codex-smoke)
if [ -n "$FLEET" ]; then
    case "$FLEET" in
        *[!0-9]*|'') echo "❌ FLEET must be a positive integer (got: $FLEET)"; exit 2 ;;
    esac
    if [ "$FLEET" -lt 1 ]; then
        echo "❌ FLEET must be >= 1 (got: $FLEET)"
        exit 2
    fi
    # Temp overlay registry: copy the checked-in stubs, add fleet manifests.
    OVERLAY="$RUN_DIR/registry-overlay"
    mkdir -p "$OVERLAY"
    cp -R "$HERE/registry/." "$OVERLAY/"
    for i in $(seq 1 "$FLEET"); do
        d="$OVERLAY/fleet-$i"
        mkdir -p "$d"
        cat > "$d/manifest.json" <<EOF
{
  "name": "fleet-$i",
  "emoji": "🧪",
  "model": "opus",
  "window": "fleet-$i",
  "role": "smoke-test",
  "kind": "claude-code",
  "enabled": true
}
EOF
        echo "You are fleet-$i, a boot-verification stub agent (aperture-xt16e). Do nothing." > "$d/prompt.md"
        AGENTS+=("fleet-$i")
    done
    REGISTRY="$OVERLAY"
    echo "🔍 fleet overlay registry: $OVERLAY (fleet-1..fleet-$FLEET)"
fi
# Exported BEFORE the hub starts: both the hub's codex-bridge discovery and
# the aperture-boot registry load must see the same stub tree.
export APERTURE_AGENTS_DIR="$REGISTRY"

# ── (a3) isolated tmux session ──
# tmux_create_window (src-tauri/src/tmux.rs) requires the target session to
# ALREADY exist — the harness owns its own throwaway session so runs never
# touch the production "aperture" session.
SMOKE_SESSION="aperture-smoke-$$"
export APERTURE_TMUX_SESSION="$SMOKE_SESSION"
tmux start-server 2>/dev/null || true
tmux new-session -d -s "$SMOKE_SESSION" -x 220 -y 50
# Panes inherit the tmux SESSION environment, not this script's exports — the
# stubs need the log dir, the hub port, and the real node path.
tmux set-environment -t "$SMOKE_SESSION" APERTURE_STUB_LOG_DIR "$APERTURE_STUB_LOG_DIR"
tmux set-environment -t "$SMOKE_SESSION" APERTURE_WS_PORT "$PORT"
tmux set-environment -t "$SMOKE_SESSION" APERTURE_REAL_NODE "$REAL_NODE"

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
    # Volta-shim belt-and-braces: kill anything still LISTENING on our run's
    # port (uniquely ours — can never touch the production hub on 4517).
    for p in $(lsof -tiTCP:"$PORT" -sTCP:LISTEN 2>/dev/null); do
        kill "$p" 2>/dev/null || true
    done
    tmux kill-session -t "$SMOKE_SESSION" 2>/dev/null || true
    # The codex stub is also spawned as the supervised app-server child of
    # aperture-boot (codex_appserver.rs honors APERTURE_CODEX_BIN); when the
    # bin exits its supervisor thread dies and the child is orphaned — reap it.
    pkill -f "$HERE/stubs/codex app-server" 2>/dev/null || true
    # Smoke agents' droppings under the REAL run dir (product convention:
    # ~/.aperture/run/<name>.{thread-id,kickoff}).
    rm -f "$HOME/.aperture/run/codex-smoke.thread-id" \
          "$HOME/.aperture/run/claude-smoke.kickoff" \
          "$HOME"/.aperture/run/fleet-*.kickoff
    wait 2>/dev/null || true
}
trap cleanup EXIT

# ── (b) start hub, wait for the listening event ──
# l2: the hub's codex-bridge discovers codex-smoke in the stub registry and
# retries binding against a sock that never speaks JSON-RPC — EXPECTED noise;
# nothing asserts on bridge events in l2. Its RUN_DIR is isolated because the
# bridge's onClose() clears <run>/<agent>.thread-id on every failed connect
# (codex-bridge.ts clearThreadReady), which would race the harness's
# pre-seeded thread-id in ~/.aperture/run. l3 keeps the default so the real
# bridge publishes the real thread-id where the pane launcher reads it.
if [ "$MODE" = "l2" ]; then
    mkdir -p "$RUN_DIR/bridge-run"
    APERTURE_WS_PORT="$PORT" APERTURE_HUB_SKIP_REPLAY=1 \
        APERTURE_RUN_DIR="$RUN_DIR/bridge-run" \
        "$REAL_NODE" "$HUB_JS" 2> "$RUN_DIR/hub.log" &
else
    APERTURE_WS_PORT="$PORT" APERTURE_HUB_SKIP_REPLAY=1 \
        "$REAL_NODE" "$HUB_JS" 2> "$RUN_DIR/hub.log" &
fi
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
# Hub-join expectations differ by mode (review finding, aperture-xt16e):
# in the REAL system the codex PANE process never talks to the hub — the
# hub's codex-bridge binds to the app-server and broadcasts the join. The
# codex stub faithfully reproduces that silence, so a codex-smoke join is
# only observable in l3 (real codex + bridge). Faking a pane-side hello would
# make the l2 green meaningless for codex. In l2, codex coverage = pane-side
# artifacts asserted after the observer verdict (section e2 below).
OBSERVER_EXPECT=(--expect-agent claude-smoke)
for a in "${AGENTS[@]}"; do
    case "$a" in
        fleet-*) OBSERVER_EXPECT+=(--expect-agent "$a") ;;
    esac
done
if [ "$MODE" = "l3" ]; then
    OBSERVER_EXPECT+=(--expect-agent codex-smoke)
    # TODO(aperture-17amw): once bridge-bind lands, add:
    #   OBSERVER_EXPECT+=(--require-bridge-bind codex-smoke)
fi
"$REAL_NODE" "$HERE/observer.mjs" \
    --port "$PORT" \
    "${OBSERVER_EXPECT[@]}" \
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
if [ "$MODE" = "l3" ]; then
    echo "🔍 observer watching for ${#OBSERVER_EXPECT[@]} expectations incl. codex-smoke join (timeout ${TIMEOUT_S}s)"
else
    echo "🔍 observer watching for claude joins; codex-smoke verified via pane artifacts (timeout ${TIMEOUT_S}s)"
fi

# ── (d) SPAWN SECTION — the real headless boot path (aperture-syepg) ──────
if [ "$MODE" = "l2" ]; then
    export APERTURE_CLAUDE_BIN="$HERE/stubs/claude"
    export APERTURE_CODEX_BIN="$HERE/stubs/codex"
    export APERTURE_LAUNCHER_PATH_PREFIX="$HERE/stubs"
fi

# Pane-side thread-id gate file. Convention (verified against master source):
# codex_appserver::socket_path = $HOME/.aperture/run/<agent>.sock, and
# launcher::build_codex_launcher derives the gate file by replacing .sock with
# .thread-id → $HOME/.aperture/run/<agent>.thread-id.
RUN_STATE_DIR="$HOME/.aperture/run"
THREAD_ID_FILE="$RUN_STATE_DIR/codex-smoke.thread-id"

if [ -n "${APERTURE_BOOT_CMD:-}" ]; then
    # Explicit override: lets us drive stubs directly (debugging) without
    # touching this script's structure.
    echo "🔍 spawning agents via APERTURE_BOOT_CMD (explicit override)"
    eval "$APERTURE_BOOT_CMD"
else
    # Resolve the headless boot bin: release, else debug, else build it.
    BIN=""
    for cand in "$REPO/src-tauri/target/release/aperture-boot" \
                "$REPO/src-tauri/target/debug/aperture-boot"; do
        if [ -x "$cand" ]; then
            BIN="$cand"
            break
        fi
    done
    if [ -z "$BIN" ]; then
        echo "🔨 aperture-boot not built — building (cd src-tauri && cargo build --bin aperture-boot)"
        if ! (cd "$REPO/src-tauri" && cargo build --bin aperture-boot); then
            echo "❌ cargo build --bin aperture-boot FAILED — cannot spawn agents"
            exit 2
        fi
        BIN="$REPO/src-tauri/target/debug/aperture-boot"
    fi

    # MODE=l2 pre-seed: no real codex-bridge will publish a thread id, so seed
    # the gate file ourselves. NOTE: boot_agent_process clears this exact file
    # as "stale" right before spawn_app_server (src-tauri/src/agents.rs), so
    # the pre-seed is deleted mid-boot — we re-seed right after the bin
    # returns (below). The pane's ≤10s dead-sock wait + ≤60s thread-id poll
    # leave ample margin. Do NOT seed in l3 (the real bridge owns the file).
    if [ "$MODE" = "l2" ]; then
        mkdir -p "$RUN_STATE_DIR"
        printf 'l2-stub-thread\n' > "$THREAD_ID_FILE"
    fi

    BOOT_ARGS=()
    for a in "${AGENTS[@]}"; do
        BOOT_ARGS+=(--agent "$a")
    done
    echo "🔍 spawning ${#AGENTS[@]} agents via $BIN"
    if ! "$BIN" "${BOOT_ARGS[@]}" > "$RUN_DIR/aperture-boot.log" 2>&1; then
        echo "❌ aperture-boot exited non-zero — $RUN_DIR/aperture-boot.log:"
        sed 's/^/   /' "$RUN_DIR/aperture-boot.log"
        exit 1
    fi
    sed 's/^/   /' "$RUN_DIR/aperture-boot.log"

    if [ "$MODE" = "l2" ]; then
        # Re-seed after the boot-time stale-file clear (see note above).
        printf 'l2-stub-thread\n' > "$THREAD_ID_FILE"
    fi
fi

# ── (e) wait for the observer verdict ──
set +e
wait "$OBS_PID"
OBS_EXIT=$?
set -e
OBS_PID=""

if [ "$OBS_EXIT" = "0" ]; then
    # ── (e2) l2 codex pane-side proof ──
    # The codex stub never joins the hub (faithful to the real pane), so l2
    # codex coverage is: the REAL spawn path invoked the stub pane with the
    # PR #37 resume-gate shape. POLL rather than instant-check: the observer
    # goes green on claude joins alone, and the codex pane spends ~10s in its
    # dead-sock wait before it execs the stub — budget 20s.
    if [ "$MODE" = "l2" ]; then
        codex_proof=0
        for _ in $(seq 1 80); do
            if [ -s "$APERTURE_STUB_LOG_DIR/codex-argv.txt" ] \
                && [ -s "$APERTURE_STUB_LOG_DIR/codex-sock.txt" ]; then
                codex_proof=1
                break
            fi
            sleep 0.25
        done
        if [ "$codex_proof" != "1" ]; then
            echo "❌ l2 codex proof missing after 20s:"
            [ -s "$APERTURE_STUB_LOG_DIR/codex-argv.txt" ] \
                || echo "   - no codex-argv.txt (codex stub never invoked)"
            [ -s "$APERTURE_STUB_LOG_DIR/codex-sock.txt" ] \
                || echo "   - no codex-sock.txt (no --remote unix:// socket in pane argv)"
            echo "   artifacts: $RUN_DIR"
            exit 1
        fi
        # PR #37 launcher shape: the pane invocation must be
        #   codex resume <thread-id> --remote unix://<sock>
        # with <thread-id> = our pre-seeded value — proving the resume gate
        # flowed through the REAL spawn path (agent_loader → boot_agent_process
        # → launcher script → pane binary) with zero harness send-keys.
        for want in "resume" "l2-stub-thread" "--remote"; do
            if ! grep -qxF -- "$want" "$APERTURE_STUB_LOG_DIR/codex-argv.txt"; then
                echo "❌ l2 codex pane argv missing '$want' — PR #37 resume-gate shape did not reach the pane binary"
                echo "   argv: $APERTURE_STUB_LOG_DIR/codex-argv.txt — artifacts: $RUN_DIR"
                exit 1
            fi
        done
        if ! grep -q '^unix://' "$APERTURE_STUB_LOG_DIR/codex-argv.txt"; then
            echo "❌ l2 codex pane argv has no 'unix://…' --remote value"
            echo "   argv: $APERTURE_STUB_LOG_DIR/codex-argv.txt — artifacts: $RUN_DIR"
            exit 1
        fi
        echo "✅ l2 codex pane proof: resume l2-stub-thread over $(tail -1 "$APERTURE_STUB_LOG_DIR/codex-sock.txt")"
    fi
    echo "✅ boot verification green (MODE=$MODE${FLEET:+ FLEET=$FLEET})"
    echo ""
    echo "── time-to-hello (ms) ──"
    "$REAL_NODE" -e '
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
if command -v tmux > /dev/null 2>&1 && tmux has-session -t "$SMOKE_SESSION" 2>/dev/null; then
    mkdir -p "$RUN_DIR/tmux"
    while IFS= read -r pane; do
        safe="$(echo "$pane" | tr ':.%' '___')"
        tmux capture-pane -p -t "$SMOKE_SESSION:$pane" > "$RUN_DIR/tmux/pane-$safe.txt" 2>/dev/null || true
    done < <(tmux list-panes -s -t "$SMOKE_SESSION" -F '#{window_index}.#{pane_index}' 2>/dev/null)
    echo "   tmux pane captures → $RUN_DIR/tmux/"
fi
# hub.log, observer.log, results.json, aperture-boot.log, stubs/*-argv.txt are
# already in RUN_DIR.
echo "❌ boot verification FAILED (MODE=$MODE${FLEET:+ FLEET=$FLEET}) — artifacts: $RUN_DIR"
echo "   hub.log observer.log results.json stubs/ $( [ -d "$RUN_DIR/tmux" ] && echo tmux/ )"
exit 1
