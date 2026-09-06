#!/usr/bin/env bash
# context-budget.sh — the §7 gates of the context diet (aperture-trgpo,
# docs/superpowers/specs/2026-09-06-context-diet-design.md). Prints a table
# and exits non-zero on any HARD breach.
#
# Measures:
#   boot_hook_bytes    scripts/aperture-prime.sh boot | wc -c      ≤ 40 KiB  (hard)
#   precompact_bytes   scripts/aperture-prime.sh precompact | wc -c ≤ 30 KiB  (hard)
#   <agent>_boot_total prompts/<a>.md + resident skill bodies (from
#                      scripts/skills-matrix.sh --json, always_injected_bytes)
#                      + boot_hook_bytes                          ≤ 120 KiB
#                      Claude: system prompt + SessionStart hook output.
#                      Codex:  prompt.md as assembled by agents.rs
#                              (inject_codex_skills + inject_bd_memory, which
#                              runs the same `aperture-prime.sh boot`).
#                      HARD for glados only today: her constitutional-core
#                      merge (orchestrator-core + resident split) lands in the
#                      same PR as this gate; the other seven are printed with
#                      PASS/FAIL for visibility and become hard as their
#                      resident sets are trimmed (spec §Migration step 5).
#   bank_bytes / bank_sha256   `bd memories --json` size + hash, so the
#                      non-destruction gate can diff before/after any tooling
#                      step. Bank CONTENT is never printed.
#
# Bytes are shown with ≈tokens (÷4). KiB throughout the labels.
# Usage: scripts/context-budget.sh [--json]     (or: just context-budget [--json])
# Exit: 0 all hard gates pass, 1 any hard gate breached, 2 usage/measurement error.
#
# Bash 3.2 compatible (macOS /bin/bash): no associative arrays, no mapfile.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRIME="$REPO/scripts/aperture-prime.sh"
MATRIX="$REPO/scripts/skills-matrix.sh"

BOOT_HOOK_MAX=$((40 * 1024))        # 40960
PRECOMPACT_MAX=$((30 * 1024))       # 30720
AGENT_BOOT_MAX=$((120 * 1024))      # 122880
HARD_AGENTS="glados"                # space-separated; grows as cores land

JSON=0
for arg in "$@"; do
    case "$arg" in
        --json) JSON=1 ;;
        -h|--help)
            sed -n '2,29p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) echo "context-budget: unknown argument '$arg' (try --json)" >&2; exit 2 ;;
    esac
done

for f in "$PRIME" "$MATRIX"; do
    if [ ! -x "$f" ]; then
        echo "context-budget: missing or non-executable $f" >&2
        exit 2
    fi
done
if ! command -v python3 >/dev/null 2>&1; then
    echo "context-budget: python3 is required to parse skills-matrix --json" >&2
    exit 2
fi

# ---------------------------------------------------------------- helpers ---

tokens() { echo $(( $1 / 4 )); }
kib() { python3 -c 'import sys; print("%.1f" % (int(sys.argv[1]) / 1024))' "$1"; }
in_list() { local x; for x in $2; do [ "$x" = "$1" ] && return 0; done; return 1; }
jstr() { printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"; }
sha256_of_stdin() {
    if command -v shasum >/dev/null 2>&1; then shasum -a 256 | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then sha256sum | awk '{print $1}'
    else echo unavailable; fi
}

# ------------------------------------------------------------- hook seams ---

# The boot seam keys "agent session" vs "operator session" on APERTURE_HUB_TOKEN_FILE (agents get the
# provisioned token FILE PATH from the launcher / inject_bd_memory; an operator shell has none and gets
# the bd workflow preamble). Measure BOTH paths EXPLICITLY and never inherit the calling shell's value —
# inheriting is how one reviewer's shell read PASS and another's FAIL on the same tree (aperture-3kavd
# HOLD #3). The agent path uses the exact path form launcher.rs exports; only the path, never contents.
TOKEN_DIR="${APERTURE_HUB_TOKEN_DIR:-$HOME/.aperture/run/hub-tokens}"
agent_boot_bytes() { env APERTURE_HUB_TOKEN_FILE="$TOKEN_DIR/$1.token" "$PRIME" boot 2>/dev/null | wc -c | tr -d ' '; }
operator_boot_bytes=$(env -u APERTURE_HUB_TOKEN_FILE "$PRIME" boot 2>/dev/null | wc -c | tr -d ' ')
precompact_bytes=$(env -u APERTURE_HUB_TOKEN_FILE "$PRIME" precompact 2>/dev/null | wc -c | tr -d ' ')
boot_hook_bytes=0   # hard gate = the LARGEST launched-agent boot hook (filled in the per-agent loop)
precompact_ok=1; [ "$precompact_bytes" -le "$PRECOMPACT_MAX" ] || precompact_ok=0

# ------------------------------------------------------------- per agent ---

# skills-matrix --json → "name<TAB>backend<TAB>prompt_bytes<TAB>resident_bytes<TAB>always_injected_bytes"
A_NAME=(); A_BACKEND=(); A_PROMPT=(); A_RES=(); A_HOOK=(); A_TOTAL=(); A_OK=(); A_HARD=()
while IFS=$'\t' read -r name backend prompt_bytes resident_bytes always; do
    [ -n "$name" ] || continue
    hook=$(agent_boot_bytes "$name")
    [ "$hook" -gt "$boot_hook_bytes" ] && boot_hook_bytes=$hook
    A_HOOK+=("$hook")
    total=$((always + hook))
    ok=1; [ "$total" -le "$AGENT_BOOT_MAX" ] || ok=0
    hard=0; in_list "$name" "$HARD_AGENTS" && hard=1
    A_NAME+=("$name"); A_BACKEND+=("$backend"); A_PROMPT+=("$prompt_bytes")
    A_RES+=("$resident_bytes"); A_TOTAL+=("$total"); A_OK+=("$ok"); A_HARD+=("$hard")
done < <("$MATRIX" --json | python3 -c '
import json, sys
d = json.load(sys.stdin)
for name in sorted(d["agents"]):
    a = d["agents"][name]
    print("\t".join(str(x) for x in (name, a["backend"], a["prompt_bytes"], a["resident_bytes"], a["always_injected_bytes"])))
')
if [ "${#A_NAME[@]}" -eq 0 ]; then
    echo "context-budget: skills-matrix --json returned no agents" >&2
    exit 2
fi
boot_hook_ok=1; [ "$boot_hook_bytes" -le "$BOOT_HOOK_MAX" ] || boot_hook_ok=0

# ------------------------------------------------------------------ bank ---

# Captured byte-exact to a temp file (a `$(...)` capture would strip the
# trailing newline), so the hash equals a by-hand
# `bd memories --json | shasum -a 256`. Content is never echoed.
bank_bytes=""; bank_sha256=""
if command -v bd >/dev/null 2>&1; then
    bank_tmp=$(mktemp "${TMPDIR:-/tmp}/context-budget-bank.XXXXXX")
    if bd memories --json >"$bank_tmp" 2>/dev/null && [ -s "$bank_tmp" ]; then
        bank_bytes=$(wc -c <"$bank_tmp" | tr -d ' ')
        bank_sha256=$(sha256_of_stdin <"$bank_tmp")
    fi
    rm -f "$bank_tmp"
fi

# ---------------------------------------------------------------- verdict ---

FAILED=""
[ "$boot_hook_ok" = 1 ] || FAILED="${FAILED:+$FAILED }boot_hook_bytes"
[ "$precompact_ok" = 1 ] || FAILED="${FAILED:+$FAILED }precompact_bytes"
for i in "${!A_NAME[@]}"; do
    if [ "${A_HARD[$i]}" = 1 ] && [ "${A_OK[$i]}" = 0 ]; then
        FAILED="${FAILED:+$FAILED }${A_NAME[$i]}_boot_total"
    fi
done
STATUS=0; [ -z "$FAILED" ] || STATUS=1

TODAY=$(date +%Y-%m-%d)
SHA=$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)

# ------------------------------------------------------------------ JSON ---

if [ "$JSON" = 1 ]; then
    bb="null"; [ -n "$bank_bytes" ] && bb="$bank_bytes"
    bs="null"; [ -n "$bank_sha256" ] && bs=$(jstr "$bank_sha256")
    printf '{\n'
    printf '  "generated": %s,\n' "$(jstr "$TODAY")"
    printf '  "repo_sha": %s,\n' "$(jstr "$SHA")"
    printf '  "status": %s,\n' "$([ "$STATUS" = 0 ] && echo '"PASS"' || echo '"FAIL"')"
    printf '  "failed": ['
    sep=""; for g in $FAILED; do printf '%s%s' "$sep" "$(jstr "$g")"; sep=", "; done
    printf '],\n'
    printf '  "hooks": {\n'
    printf '    "boot_hook_bytes": {"bytes": %s, "tokens": %s, "max_bytes": %s, "pass": %s, "hard": true},\n' \
        "$boot_hook_bytes" "$(tokens "$boot_hook_bytes")" "$BOOT_HOOK_MAX" "$([ "$boot_hook_ok" = 1 ] && echo true || echo false)"
    printf '    "precompact_bytes": {"bytes": %s, "tokens": %s, "max_bytes": %s, "pass": %s, "hard": true},\n' \
        "$precompact_bytes" "$(tokens "$precompact_bytes")" "$PRECOMPACT_MAX" "$([ "$precompact_ok" = 1 ] && echo true || echo false)"
    printf '    "operator_boot_bytes": {"bytes": %s, "tokens": %s, "path": "no APERTURE_HUB_TOKEN_FILE (operator shell, includes bd workflow preamble)", "hard": false}\n' \
        "$operator_boot_bytes" "$(tokens "$operator_boot_bytes")"
    printf '  },\n'
    printf '  "agents": {'
    sep=""
    for i in "${!A_NAME[@]}"; do
        printf '%s\n    %s: {"backend": %s, "prompt_bytes": %s, "resident_bytes": %s, "boot_hook_bytes": %s, "boot_total_bytes": %s, "tokens": %s, "max_bytes": %s, "pass": %s, "hard": %s}' \
            "$sep" "$(jstr "${A_NAME[$i]}")" "$(jstr "${A_BACKEND[$i]}")" "${A_PROMPT[$i]}" "${A_RES[$i]}" \
            "${A_HOOK[$i]}" "${A_TOTAL[$i]}" "$(tokens "${A_TOTAL[$i]}")" "$AGENT_BOOT_MAX" \
            "$([ "${A_OK[$i]}" = 1 ] && echo true || echo false)" "$([ "${A_HARD[$i]}" = 1 ] && echo true || echo false)"
        sep=","
    done
    printf '\n  },\n'
    printf '  "bank": {"bank_bytes": %s, "bank_sha256": %s}\n' "$bb" "$bs"
    printf '}\n'
    exit "$STATUS"
fi

# ----------------------------------------------------------------- table ---

verdict() { # verdict ok hard
    if [ "$1" = 1 ]; then echo PASS
    elif [ "$2" = 1 ]; then echo FAIL
    else echo "FAIL (soft)"; fi
}

echo "Aperture context budget — $TODAY (repo: $SHA)"
echo

echo "INJECTION SEAMS (scripts/aperture-prime.sh)"
printf '%-18s %9s %9s %9s %8s   %s\n' gate bytes KiB '≈tokens' 'max KiB' verdict
printf '%-18s %9s %9s %9s %8s   %s\n' boot_hook_bytes "$boot_hook_bytes" "$(kib "$boot_hook_bytes")" "$(tokens "$boot_hook_bytes")" "$((BOOT_HOOK_MAX / 1024))" "$(verdict "$boot_hook_ok" 1)"
printf '%-18s %9s %9s %9s %8s   %s\n' precompact_bytes "$precompact_bytes" "$(kib "$precompact_bytes")" "$(tokens "$precompact_bytes")" "$((PRECOMPACT_MAX / 1024))" "$(verdict "$precompact_ok" 1)"
printf '%-18s %9s %9s %9s %8s   %s\n' operator_boot "$operator_boot_bytes" "$(kib "$operator_boot_bytes")" "$(tokens "$operator_boot_bytes")" "-" "info (no token file: operator shell, bd preamble included; not gated)"
printf '%-18s %s\n' '' "boot_hook_bytes = largest launched-agent boot hook, measured with APERTURE_HUB_TOKEN_FILE=$TOKEN_DIR/<agent>.token (launcher.rs form); never inherited from this shell"
echo

echo "ASSEMBLED BOOT PROMPT per agent (prompt + resident skills + boot hook)  max $((AGENT_BOOT_MAX / 1024)) KiB"
printf '%-9s %-7s %8s %9s %9s %10s %9s %9s   %s\n' agent path prompt resident boot_hook total 'KiB' '≈tokens' verdict
for i in "${!A_NAME[@]}"; do
    printf '%-9s %-7s %8s %9s %9s %10s %9s %9s   %s\n' \
        "${A_NAME[$i]}" "${A_BACKEND[$i]}" "${A_PROMPT[$i]}" "${A_RES[$i]}" "${A_HOOK[$i]}" \
        "${A_TOTAL[$i]}" "$(kib "${A_TOTAL[$i]}")" "$(tokens "${A_TOTAL[$i]}")" "$(verdict "${A_OK[$i]}" "${A_HARD[$i]}")"
done
echo "(hard gate: $HARD_AGENTS — the constitutional core lands with this gate; the rest are soft until their"
echo " resident sets are trimmed. claude = prompt + SessionStart hook; codex = prompt.md via inject_bd_memory.)"
echo

echo "MEMORY BANK (non-destruction reference — content never printed)"
if [ -n "$bank_bytes" ]; then
    printf '%-12s %s (%s KiB)\n' bank_bytes "$bank_bytes" "$(kib "$bank_bytes")"
    printf '%-12s %s\n' bank_sha256 "$bank_sha256"
else
    echo "bank_bytes   unavailable (bd memories --json failed — not a gate)"
fi
echo

if [ "$STATUS" = 0 ]; then
    echo "RESULT: PASS"
else
    echo "RESULT: FAIL — $FAILED"
fi
exit "$STATUS"
