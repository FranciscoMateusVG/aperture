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
# Portable real deadline (GLaDOS hold #1): macOS ships neither `timeout` nor `gtimeout`, and an
# unbounded `bd prime` would make boot unbounded. Run the command in the background, arm a
# watchdog subshell that SIGTERMs then SIGKILLs it at the deadline, and reap. The watchdog's
# stdio is detached so a $(…) capture around this function returns as soon as the child dies.
# Exit 124 on deadline, else the child's own status. Verified by test/aperture-prime.test.mjs:
# the hung child is gone (kill -0 fails), not merely the wrapper returned.
run_with_timeout() {
    local secs="$1"; shift
    local pid wd rc
    "$@" &
    pid=$!
    ( sleep "$secs"; kill -TERM "$pid" 2>/dev/null; sleep 1; kill -KILL "$pid" 2>/dev/null ) >/dev/null 2>&1 </dev/null &
    wd=$!
    wait "$pid" 2>/dev/null; rc=$?
    kill "$wd" 2>/dev/null; wait "$wd" 2>/dev/null
    if kill -0 "$pid" 2>/dev/null; then kill -KILL "$pid" 2>/dev/null; fi
    if [ "$rc" -eq 143 ] || [ "$rc" -eq 137 ]; then return 124; fi
    return "$rc"
}
is_blank() { [ -z "$(printf '%s' "$1" | tr -d '[:space:]')" ]; }
byte_len() { printf '%s' "$1" | wc -c | tr -d ' '; }

# ------------------------------------------------------------ preamble ---

# `bd prime` minus its memory section. The section starts at the line
# `## Persistent Memories (N)` and runs to EOF; awk stops printing there.
# GLaDOS holds #2/#3: never advise re-running `bd prime` (that re-injects the whole, possibly
# secret-bearing bank), and never trust a regex + size cap alone — emit the preamble ONLY when
# the stripped text has the recognised structure: first line `# Beads Workflow Context`, the
# `## Persistent Memories` header was actually found (so stripping happened), a workflow
# section is present, and no memory-entry heading (`### <key>`) survives. Anything else is
# suppressed with a one-line notice. Memory always goes through recall/recall_full.
PREAMBLE_UNAVAILABLE="[bd workflow preamble unavailable — bd usage is in the beads skill; memory via recall/recall_full]"
print_preamble() {
    local raw rc stripped found first
    if ! command -v bd >/dev/null 2>&1; then echo "$PREAMBLE_UNAVAILABLE"; return; fi
    raw=$(run_with_timeout "$BD_TIMEOUT_SECS" bd prime 2>/dev/null); rc=$?
    if [ "$rc" -ne 0 ] || is_blank "$raw"; then echo "$PREAMBLE_UNAVAILABLE"; return; fi
    found=$(printf '%s\n' "$raw" | grep -c "$MEMORY_HEADER_RE")
    stripped=$(printf '%s\n' "$raw" | awk -v re="$MEMORY_HEADER_RE" '$0 ~ re { exit } { print }')
    first=$(printf '%s\n' "$stripped" | head -n 1)
    if [ "$found" -lt 1 ] \
       || [ "$first" != "# Beads Workflow Context" ] \
       || ! printf '%s\n' "$stripped" | grep -qE '^## (Core Rules|Essential Commands)' \
       || printf '%s\n' "$stripped" | grep -qE '^### [A-Za-z0-9][A-Za-z0-9._-]{5,}$' \
       || [ "$(byte_len "$stripped")" -gt "$PREAMBLE_MAX_BYTES" ]; then
        echo "[bd workflow preamble suppressed — unrecognised bd prime structure; bd usage is in the beads skill; memory via recall/recall_full]"
        return
    fi
    printf '%s\n' "$stripped"
}
print_index() {
    local mode="$1" out rc
    if ! command -v node >/dev/null 2>&1; then
        echo "[memory index unavailable: node not found — use recall/recall_full]"
        return
    fi
    if [ ! -f "$INDEX_JS" ]; then
        echo "[memory index unavailable: dist not built (just build-mcp) — use recall/recall_full]"
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
        # Aperture agents (APERTURE_HUB_TOKEN_FILE set by the launcher) carry the resident
        # `beads` skill, which documents every bd command the preamble repeats — skip it for
        # them (≈4.7 KiB of boot budget). The operator's own plain sessions in this repo keep it.
        if [ -n "${APERTURE_HUB_TOKEN_FILE:-}" ]; then
            echo "[bd workflow preamble omitted for Aperture agents — the resident beads skill covers bd usage; memory via recall/recall_full]"
        else
            print_preamble
        fi
        echo
        print_index boot
        ;;
    precompact)
        print_index precompact
        ;;
esac
exit 0
