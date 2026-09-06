#!/usr/bin/env bash
# skills-matrix.sh — report what each Aperture agent actually carries at boot.
#
# Walks the canonical registry (agents/<name>/{manifest.json,skills.txt,
# resident.txt}, prompts/<name>.md, .claude/skills/<skill>/SKILL.md) plus the
# runtime model overrides in ~/.aperture/agent-config.json and prints, per
# agent, which skills are RESIDENT (full body force-injected into the boot
# prompt) vs LAZY (name+description only, body read on demand), with byte
# sizes. Mirrors the injection rule in src-tauri/src/agents.rs:
#
#   effective model = override (agent-config.json) > manifest.json "model"
#   codex/*  → resident = resident.txt (or ALL of skills.txt when absent),
#              lazy = the rest of skills.txt via $CODEX_HOME/skills
#   else     → resident = ALL of skills.txt; everything else in
#              .claude/skills/ is lazily discoverable by Claude Code natively
#
# Usage: scripts/skills-matrix.sh [--json]     (or: just skills-matrix [--json])
#
# Bash 3.2 compatible (macOS /bin/bash): no associative arrays, no mapfile.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKILLS_DIR="$REPO/.claude/skills"
AGENTS_DIR="$REPO/agents"
PROMPTS_DIR="$REPO/prompts"
CONFIG="${APERTURE_AGENT_CONFIG:-$HOME/.aperture/agent-config.json}"

JSON=0
for arg in "$@"; do
    case "$arg" in
        --json) JSON=1 ;;
        -h|--help)
            sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) echo "skills-matrix: unknown argument '$arg' (try --json)" >&2; exit 2 ;;
    esac
done

# ---------------------------------------------------------------- helpers ---

# Same line convention as `just setup` / agent_loader::parse_skill_lines:
# strip `#` comments (inline or full-line), trim, drop blanks.
parse_lines() {
    sed -e 's/#.*//' -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' "$1" | sed '/^$/d'
}

file_bytes() { wc -c < "$1" | tr -d ' '; }

# Read "model" from a manifest.json.
manifest_model() {
    if command -v python3 >/dev/null 2>&1; then
        python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("model",""))' "$1"
    else
        sed -n 's/.*"model"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$1" | head -1
    fi
}

# Emit "agent<TAB>model" lines from agent-config.json (flat {name: model} map).
read_overrides() {
    [ -f "$CONFIG" ] || return 0
    if command -v python3 >/dev/null 2>&1; then
        python3 -c '
import json,sys
try:
    d = json.load(open(sys.argv[1]))
except Exception as e:
    sys.stderr.write("skills-matrix: cannot parse %s: %s\n" % (sys.argv[1], e)); sys.exit(0)
for k, v in (d.items() if isinstance(d, dict) else []):
    if isinstance(v, str): print("%s\t%s" % (k, v))
' "$CONFIG"
    else
        sed -n 's/^[[:space:]]*"\([^"]*\)"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1\t\2/p' "$CONFIG"
    fi
}

# Space-separated list membership / set ops.
in_list() { # in_list needle "a b c"
    local x
    for x in $2; do [ "$x" = "$1" ] && return 0; done
    return 1
}
add_unique() { # add_unique "list" item  -> prints new list
    if in_list "$2" "$1"; then printf '%s' "$1"; else printf '%s' "${1:+$1 }$2"; fi
}

# JSON string escaping (names/models are plain identifiers; be safe anyway).
jstr() { printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"; }
jlist() { # jlist "a b c" -> ["a","b","c"]
    local out="" x
    for x in $1; do out="${out:+$out,}$(jstr "$x")"; done
    printf '[%s]' "$out"
}

# ------------------------------------------------------ skill catalogue ---

SK_NAMES=()   # skill name
SK_BYTES=()   # SKILL.md size
SK_PATHS=()   # repo-relative path to the SKILL.md actually found

for skill_dir in "$SKILLS_DIR"/*/; do   # trailing slash follows symlinked dirs, like `just setup`
    [ -d "$skill_dir" ] || continue
    name=$(basename "$skill_dir")
    # Case-insensitive, first match only: APFS is case-insensitive, so
    # `cat SKILL.md skill.md` would double-count the same file.
    md=$(find "$skill_dir" -maxdepth 1 -iname 'SKILL.md' 2>/dev/null | head -1)
    [ -n "$md" ] || continue
    SK_NAMES+=("$name")
    SK_BYTES+=("$(file_bytes "$md")")
    SK_PATHS+=("${md#"$REPO"/}")
done

skill_index() { # -> prints index or nothing
    local i
    for i in "${!SK_NAMES[@]}"; do
        [ "${SK_NAMES[$i]}" = "$1" ] && { printf '%s' "$i"; return 0; }
    done
    return 1
}
skill_bytes() { local i; i=$(skill_index "$1") && printf '%s' "${SK_BYTES[$i]}"; }
sum_bytes() { # sum_bytes "a b c" -> total of known skills
    local t=0 x b
    for x in $1; do b=$(skill_bytes "$x" || true); t=$((t + ${b:-0})); done
    printf '%s' "$t"
}

# ------------------------------------------------------ runtime overrides ---

OV_NAMES=()
OV_MODELS=()
while IFS=$'\t' read -r k v; do
    [ -n "$k" ] || continue
    OV_NAMES+=("$k"); OV_MODELS+=("$v")
done < <(read_overrides)
CONFIG_PRESENT=0; [ -f "$CONFIG" ] && CONFIG_PRESENT=1

override_for() {
    local i
    for i in ${OV_NAMES[@]+"${!OV_NAMES[@]}"}; do
        [ "${OV_NAMES[$i]}" = "$1" ] && { printf '%s' "${OV_MODELS[$i]}"; return 0; }
    done
    return 1
}

# --------------------------------------------------------------- agents ---

A_NAME=(); A_MANIFEST=(); A_OVERRIDE=(); A_EFF=(); A_BACKEND=(); A_PROMPT=()
A_RES=(); A_LAZY=(); A_RESB=(); A_LAZYB=(); A_HASRES=()
MISSING=""          # "skill" names referenced anywhere but absent from the catalogue
MISSING_BY=""       # "skill:agent" pairs
RES_NOT_IN_SKILLS="" # "agent:skill" — resident.txt entry not in skills.txt (runtime warns + skips)

for agent_dir in "$AGENTS_DIR"/*/; do
    [ -d "$agent_dir" ] || continue
    name=$(basename "$agent_dir")
    manifest="$agent_dir/manifest.json"
    [ -f "$manifest" ] || continue   # `just setup` skips these too

    mmodel=$(manifest_model "$manifest")
    override=$(override_for "$name" || true)
    eff="${override:-$mmodel}"
    case "$eff" in codex/*) backend=codex ;; *) backend=claude ;; esac

    prompt_bytes=0
    [ -f "$PROMPTS_DIR/$name.md" ] && prompt_bytes=$(file_bytes "$PROMPTS_DIR/$name.md")

    # skills.txt → assigned (existing) list; unknown names → MISSING.
    assigned=""
    if [ -f "$agent_dir/skills.txt" ]; then
        while IFS= read -r s; do
            if skill_index "$s" >/dev/null; then
                assigned=$(add_unique "$assigned" "$s")
            else
                MISSING=$(add_unique "$MISSING" "$s")
                MISSING_BY=$(add_unique "$MISSING_BY" "$s:$name")
            fi
        done < <(parse_lines "$agent_dir/skills.txt")
    fi

    # resident.txt (optional) → requested resident list.
    hasres=0; requested=""
    if [ -f "$agent_dir/resident.txt" ]; then
        hasres=1
        while IFS= read -r s; do
            requested=$(add_unique "$requested" "$s")
            if ! skill_index "$s" >/dev/null; then
                MISSING=$(add_unique "$MISSING" "$s")
                MISSING_BY=$(add_unique "$MISSING_BY" "$s:$name")
            elif ! in_list "$s" "$assigned"; then
                RES_NOT_IN_SKILLS=$(add_unique "$RES_NOT_IN_SKILLS" "$name:$s")
            fi
        done < <(parse_lines "$agent_dir/resident.txt")
    fi

    # Injection rule (src-tauri/src/agents.rs inject_skills / inject_codex_skills).
    resident=""; lazy=""
    if [ "$backend" = codex ] && [ "$hasres" = 1 ]; then
        for s in $assigned; do
            if in_list "$s" "$requested"; then resident=$(add_unique "$resident" "$s")
            else lazy=$(add_unique "$lazy" "$s"); fi
        done
    else
        resident="$assigned"
    fi

    A_NAME+=("$name"); A_MANIFEST+=("$mmodel"); A_OVERRIDE+=("$override")
    A_EFF+=("$eff"); A_BACKEND+=("$backend"); A_PROMPT+=("$prompt_bytes")
    A_RES+=("$resident"); A_LAZY+=("$lazy"); A_HASRES+=("$hasres")
    A_RESB+=("$(sum_bytes "$resident")"); A_LAZYB+=("$(sum_bytes "$lazy")")
done

# Skills assigned to at least one agent, and the unassigned remainder.
ASSIGNED_ANY=""
for i in ${A_NAME[@]+"${!A_NAME[@]}"}; do
    for s in ${A_RES[$i]} ${A_LAZY[$i]}; do ASSIGNED_ANY=$(add_unique "$ASSIGNED_ANY" "$s"); done
done
UNASSIGNED=""
for s in ${SK_NAMES[@]+"${SK_NAMES[@]}"}; do
    in_list "$s" "$ASSIGNED_ANY" || UNASSIGNED=$(add_unique "$UNASSIGNED" "$s")
done

# Assigned skills sorted by bytes desc (ties by name).
SORTED=""
if [ -n "$ASSIGNED_ANY" ]; then
    SORTED=$(for s in $ASSIGNED_ANY; do printf '%s %s\n' "$(skill_bytes "$s")" "$s"; done \
             | sort -k1,1nr -k2,2 | awk '{print $2}' | tr '\n' ' ')
fi

TODAY=$(date +%Y-%m-%d)
SHA=$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)

# ----------------------------------------------------------------- JSON ---

if [ "$JSON" = 1 ]; then
    printf '{\n'
    printf '  "generated": %s,\n' "$(jstr "$TODAY")"
    printf '  "repo_sha": %s,\n' "$(jstr "$SHA")"
    printf '  "config_path": %s,\n' "$(jstr "$CONFIG")"
    printf '  "config_present": %s,\n' "$([ "$CONFIG_PRESENT" = 1 ] && echo true || echo false)"
    printf '  "skills": {'
    sep=""
    for i in ${SK_NAMES[@]+"${!SK_NAMES[@]}"}; do
        printf '%s\n    %s: {"bytes": %s, "path": %s}' "$sep" "$(jstr "${SK_NAMES[$i]}")" "${SK_BYTES[$i]}" "$(jstr "${SK_PATHS[$i]}")"
        sep=","
    done
    printf '\n  },\n  "agents": {'
    sep=""
    for i in ${A_NAME[@]+"${!A_NAME[@]}"}; do
        ov="null"; [ -n "${A_OVERRIDE[$i]}" ] && ov=$(jstr "${A_OVERRIDE[$i]}")
        printf '%s\n    %s: {' "$sep" "$(jstr "${A_NAME[$i]}")"
        printf '"manifest_model": %s, "override": %s, "effective_model": %s, "backend": %s, ' \
            "$(jstr "${A_MANIFEST[$i]}")" "$ov" "$(jstr "${A_EFF[$i]}")" "$(jstr "${A_BACKEND[$i]}")"
        printf '"has_resident_txt": %s, "prompt_bytes": %s, ' \
            "$([ "${A_HASRES[$i]}" = 1 ] && echo true || echo false)" "${A_PROMPT[$i]}"
        printf '"resident": %s, "resident_bytes": %s, "lazy": %s, "lazy_bytes": %s, "always_injected_bytes": %s}' \
            "$(jlist "${A_RES[$i]}")" "${A_RESB[$i]}" "$(jlist "${A_LAZY[$i]}")" "${A_LAZYB[$i]}" \
            "$(( A_PROMPT[i] + A_RESB[i] ))"
        sep=","
    done
    printf '\n  },\n  "unassigned": ['
    sep=""
    for s in $UNASSIGNED; do
        printf '%s\n    {"name": %s, "bytes": %s}' "$sep" "$(jstr "$s")" "$(skill_bytes "$s")"; sep=","
    done
    printf '\n  ],\n  "missing": ['
    sep=""
    for s in $MISSING; do
        by=""
        for pair in $MISSING_BY; do [ "${pair%%:*}" = "$s" ] && by=$(add_unique "$by" "${pair#*:}"); done
        printf '%s\n    {"name": %s, "referenced_by": %s}' "$sep" "$(jstr "$s")" "$(jlist "$by")"; sep=","
    done
    printf '\n  ],\n  "resident_not_in_skills": ['
    sep=""
    for pair in $RES_NOT_IN_SKILLS; do
        printf '%s\n    {"agent": %s, "skill": %s}' "$sep" "$(jstr "${pair%%:*}")" "$(jstr "${pair#*:}")"; sep=","
    done
    printf '\n  ]\n}\n'
    exit 0
fi

# ---------------------------------------------------------------- tables ---

echo "Aperture skills matrix — $TODAY (repo: $SHA)"
echo

echo "PER-AGENT INJECTED CONTEXT"
printf '%-9s %-20s %-7s %8s %7s %6s %7s   %s\n' agent model path resident bytes lazy bytes 'always-injected (prompt+resident)'
for i in ${A_NAME[@]+"${!A_NAME[@]}"}; do
    star=""; [ -n "${A_OVERRIDE[$i]}" ] && star="*"
    rc=0; for s in ${A_RES[$i]}; do rc=$((rc + 1)); done
    lc=0; for s in ${A_LAZY[$i]}; do lc=$((lc + 1)); done
    printf '%-9s %-20s %-7s %8s %7s %6s %7s   %s\n' \
        "${A_NAME[$i]}" "${A_EFF[$i]}$star" "${A_BACKEND[$i]}" "$rc" "${A_RESB[$i]}" "$lc" "${A_LAZYB[$i]}" \
        "$(( A_PROMPT[i] + A_RESB[i] ))"
done
if [ "$CONFIG_PRESENT" = 1 ]; then
    echo "(* = runtime override from $CONFIG; ÷4 ≈ tokens)"
else
    echo "(no runtime overrides: $CONFIG not found; ÷4 ≈ tokens)"
fi
echo

echo "SKILL × AGENT  (R = resident, L = lazy, . = not assigned)"
line=$(printf '%-34s %6s ' skill bytes)
for i in ${A_NAME[@]+"${!A_NAME[@]}"}; do line="$line$(printf ' %-8s' "${A_NAME[$i]}")"; done
echo "${line%"${line##*[! ]}"}"
for s in $SORTED; do
    line=$(printf '%-34s %6s ' "$s" "$(skill_bytes "$s")")
    for i in ${A_NAME[@]+"${!A_NAME[@]}"}; do
        if in_list "$s" "${A_RES[$i]}"; then m=R
        elif in_list "$s" "${A_LAZY[$i]}"; then m=L
        else m=.; fi
        line="$line$(printf ' %-8s' "$m")"
    done
    echo "${line%"${line##*[! ]}"}"   # strip trailing spaces
done
echo "sorted by bytes desc"
echo

printf 'UNASSIGNED SKILLS (in .claude/skills but in no agent'"'"'s skills.txt): '
if [ -z "$UNASSIGNED" ]; then echo none; else
    echo
    for s in $UNASSIGNED; do printf '  %-34s %6s\n' "$s" "$(skill_bytes "$s")"; done
fi

printf 'MISSING SKILLS (named in a skills.txt/resident.txt but no .claude/skills/<name>/SKILL.md): '
if [ -z "$MISSING" ]; then echo none; else
    echo
    for s in $MISSING; do
        by=""
        for pair in $MISSING_BY; do [ "${pair%%:*}" = "$s" ] && by="${by:+$by, }${pair#*:}"; done
        printf '  %-34s referenced by: %s\n' "$s" "$by"
    done
fi

if [ -n "$RES_NOT_IN_SKILLS" ]; then
    echo "RESIDENT NOT IN skills.txt (runtime warns and skips these — add them to skills.txt or drop from resident.txt):"
    for pair in $RES_NOT_IN_SKILLS; do printf '  %-9s %s\n' "${pair%%:*}" "${pair#*:}"; done
fi
