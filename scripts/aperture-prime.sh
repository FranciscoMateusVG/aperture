#!/usr/bin/env bash
# aperture-prime.sh — the two context-injection seams for the context diet
# (aperture-trgpo, docs/superpowers/specs/2026-09-06-context-diet-design.md §5).
#
# Replaces bare `bd prime` everywhere it used to be injected whole (~377 KiB:
# 4.6 KiB workflow preamble + ~373 KiB memory bank). Two explicit modes, no
# full-bank fallback — the memory bank is NEVER printed by this script; agents
# reach it lazily via the aperture-bus `recall` / `recall_full` tools.
#
#   boot        SessionStart hook (Claude) and agents.rs::inject_bd_memory
#               (Codex). Prints `bd prime` with everything from the
#               `## Persistent Memories` line to EOF stripped, then the memory
#               index block from `node dist/memory-index.js --mode boot`
#               (standing decisions inlined + one index line per live entry).
#               Budget: ≤ 40 KiB total.
#   precompact  PreCompact hook. ONLY the index block (`--mode precompact`),
#               no workflow preamble.  Budget: ≤ 30 KiB.
#
# Failure policy (spec §5): on any failure — bd down, dist not built, node
# missing, CLI error/empty — print a one-line `[... unavailable: <reason>]`
# marker in place of the missing block and still exit 0, so the absence is
# visible in context and the hook/agent boot never fails hard. The index CLI
# handles its own bd failures (last-good standing cache); this script only
# guards the seams around it.
#
# Safety caps (belt and braces, never the gate — `just context-budget` is):
#   - stripped preamble > 16 KiB  → suppressed (a bank leak, not a preamble)
#   - index block      > 64 KiB  → suppressed (CLI regression, not an index)
#
# Env: APERTURE_MCP_DIST overrides <repo>/mcp-server/dist.
# Usage: scripts/aperture-prime.sh <boot|precompact>   (or: just aperture-prime MODE)
#
# Bash 3.2 compatible (macOS /bin/bash). Deliberately NOT `set -e`.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MCP_DIST="${APERTURE_MCP_DIST:-$REPO/mcp-server/dist}"
INDEX_JS="$MCP_DIST/memory-index.js"

BD_TIMEOUT_SECS=5
NODE_TIMEOUT_SECS=15
PREAMBLE_MAX_BYTES=$((16 * 1024))
INDEX_MAX_BYTES=$((64 * 1024))
MEMORY_HEADER_RE='^## Persistent Memories'

MODE="${1:-}"
case "$MODE" in
    boot|precompact) ;;
    -h|--help)
        sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'
        exit 0 ;;
    *)
        echo "aperture-prime: expected one of boot|precompact, got '${MODE}'" >&2
        exit 2 ;;
esac

# ---------------------------------------------------------------- helpers ---

# run_with_timeout SECS cmd...  — uses coreutils timeout/gtimeout when present
# (macOS ships neither by default); otherwise runs the command plain.
run_with_timeout() {
    local secs="$1"; shift
    if command -v timeout >/dev/null 2>&1; then
        timeout "$secs" "$@"
    elif command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$secs" "$@"
    else
        "$@"
    fi
}

is_blank() { [ -z "$(printf '%s' "$1" | tr -d '[:space:]')" ]; }
byte_len() { printf '%s' "$1" | wc -c | tr -d ' '; }

# ------------------------------------------------------------ preamble ---

# `bd prime` minus its memory section. The section starts at the line
# `## Persistent Memories (N)` and runs to EOF; awk stops printing there.
print_preamble() {
    local raw rc stripped
    if ! command -v bd >/dev/null 2>&1; then
        echo "[bd prime unavailable — run bd prime manually]"
        return
    fi
    raw=$(run_with_timeout "$BD_TIMEOUT_SECS" bd prime 2>/dev/null); rc=$?
    if [ "$rc" -ne 0 ] || is_blank "$raw"; then
        echo "[bd prime unavailable — run bd prime manually]"
        return
    fi
    stripped=$(printf '%s\n' "$raw" | awk -v re="$MEMORY_HEADER_RE" '$0 ~ re { exit } { print }')
    if is_blank "$stripped"; then
        echo "[bd prime unavailable — run bd prime manually]"
        return
    fi
    if [ "$(byte_len "$stripped")" -gt "$PREAMBLE_MAX_BYTES" ]; then
        # The preamble is ~5 KiB; anything this big means the memory header
        # moved and the bank slipped through. Never print it.
        echo "[bd prime preamble exceeded $((PREAMBLE_MAX_BYTES / 1024)) KiB after stripping — suppressed; run bd prime manually]"
        return
    fi
    printf '%s\n' "$stripped"
}

# --------------------------------------------------------- memory index ---

# The index CLI contract (mcp-server/src/memory-index.ts): prints the
# rendered block, exit 0 always, fallback text on failure. Until it lands,
# dist/memory-index.js is absent or a stub — both are treated as "unavailable".
print_index() {
    local mode="$1" out rc
    if ! command -v node >/dev/null 2>&1; then
        echo "[memory index unavailable: node not found — use recall/recall_full]"
        return
    fi
    if [ ! -f "$INDEX_JS" ]; then
        echo "[memory index unavailable: dist not built — run just build-mcp]"
        return
    fi
    out=$(run_with_timeout "$NODE_TIMEOUT_SECS" node "$INDEX_JS" --mode "$mode" 2>/dev/null); rc=$?
    if [ "$rc" -ne 0 ]; then
        echo "[memory index unavailable: memory-index.js --mode $mode exited $rc — use recall/recall_full]"
        return
    fi
    if is_blank "$out"; then
        echo "[memory index unavailable: memory-index.js --mode $mode printed nothing — use recall/recall_full]"
        return
    fi
    if [ "$(byte_len "$out")" -gt "$INDEX_MAX_BYTES" ]; then
        echo "[memory index unavailable: --mode $mode output exceeded $((INDEX_MAX_BYTES / 1024)) KiB safety cap — use recall/recall_full]"
        return
    fi
    printf '%s\n' "$out"
}

# ------------------------------------------------------------------ main ---

case "$MODE" in
    boot)
        print_preamble
        echo
        print_index boot
        ;;
    precompact)
        print_index precompact
        ;;
esac
exit 0
