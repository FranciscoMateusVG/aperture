#!/usr/bin/env bash
# retention-gate — §7 standing-rule retention (aperture-trgpo, GLaDOS holds #4/#4b).
#   1. Every DECISION-n rule sentence in orchestrator-core/DECISIONS.md is present VERBATIM in the
#      resident artifact orchestrator-core/SKILL.md (the only file the injectors append), and
#      orchestrator-core is in GLaDOS's resident set on both paths.
#   2. Every sidecar entry designated standing:true is rendered in BOTH aperture-prime.sh modes
#      (boot + precompact), and the standing block is not over budget. Omission = FAIL, never a
#      silent demotion.
# Exit 1 on any breach.
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEC="$REPO/.claude/skills/orchestrator-core/DECISIONS.md"
SKILL="$REPO/.claude/skills/orchestrator-core/SKILL.md"
SEED="${APERTURE_MEMORY_META:-$HOME/.aperture/memory-meta.json}"
[ -f "$SEED" ] || SEED="$REPO/docs/memory-meta.seed.json"
fail=0; n=0; ok=0; VERIFIED=""
while IFS= read -r line; do
  id=$(printf '%s' "$line" | awk -F' \\| ' '{print $1}' | sed 's/^| *//; s/ *$//')
  rule=$(printf '%s' "$line" | awk -F' \\| ' '{print $2}')
  n=$((n+1))
  if grep -qF -- "$rule" "$SKILL"; then ok=$((ok+1)); VERIFIED="${VERIFIED:+$VERIFIED, }$id"; else echo "MISSING in SKILL.md: $id"; fail=1; fi
done < <(grep '^| DECISION-' "$DEC")
# Named coverage: the DECISION ids in DECISIONS.md must equal the ids the design spec enumerates (§7),
# so "how many rules must be resident" is a named list in both places, never a bare count that can drift
# (aperture-3kavd: spec said 25, gate enforced 13 — the 25 was an unenumerated draft estimate).
SPEC="$REPO/docs/superpowers/specs/2026-09-06-context-diet-design.md"
dec_ids=$(grep '^| DECISION-' "$DEC" | awk -F' \\| ' '{print $1}' | sed 's/^| *//; s/ *$//' | sort)
spec_ids=$(grep 'enumerated DECISION rows' "$SPEC" | grep -oE 'DECISION-[0-9]+b?' | sort -u)
if [ "$dec_ids" != "$spec_ids" ]; then
  echo "DECISION id set differs between DECISIONS.md and the spec §7:"; diff <(echo "$dec_ids") <(echo "$spec_ids") | sed 's/^/  /'; fail=1
fi
echo "retention-gate[decisions] verified ids: ${VERIFIED:-none}"
for f in "$REPO/agents/glados/resident.txt" "$REPO/agents/glados/skills.txt"; do
  grep -qx 'orchestrator-core' "$f" || { echo "orchestrator-core not in $(basename "$f")"; fail=1; }
done
grep -q '^## 0. Binding decisions' "$SKILL" || { echo "SKILL.md lacks the binding-decisions section"; fail=1; }
echo "retention-gate[decisions]: $ok/$n DECISION rules resident in SKILL.md"

# standing memories: every designated key must appear in both rendered modes
mapfile -t STANDING < <(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print('\n'.join(k for k,v in sorted(d.items()) if v.get('standing')))" "$SEED")
sn=${#STANDING[@]}; sboot=0; spre=0
BOOT=$(APERTURE_HUB_TOKEN_FILE="${APERTURE_HUB_TOKEN_FILE:-/dev/null}" "$REPO/scripts/aperture-prime.sh" boot)
PRE=$("$REPO/scripts/aperture-prime.sh" precompact)
for k in "${STANDING[@]}"; do
  printf '%s\n' "$BOOT" | grep -qF -- "- **$k**" && sboot=$((sboot+1)) || { echo "standing NOT resident in boot: $k"; fail=1; }
  printf '%s\n' "$PRE"  | grep -qF -- "- **$k**" && spre=$((spre+1))  || { echo "standing NOT resident in precompact: $k"; fail=1; }
done
for label in BOOT PRE; do
  printf '%s\n' "${!label}" | grep -q 'STANDING BLOCK OVER BUDGET' && { echo "standing block over budget in $label"; fail=1; }
  printf '%s\n' "${!label}" | grep -q 'unreviewed — full memory body' && echo "note: unreviewed standing statement(s) rendered as full body in $label (allowed, review pending)"
done
echo "retention-gate[standing] designated keys (separate set from DECISION rows): $(printf '%s, ' "${STANDING[@]}" | sed 's/, $//')"
echo "retention-gate[standing]: boot $sboot/$sn, precompact $spre/$sn designated standing statements resident"
# constitution: every C-n rule sentence in constitution/DECISIONS.md must be present VERBATIM in the
# resident constitution/SKILL.md, and the pilot agent (peppy) must carry `constitution` in resident.txt
# (aperture-g4hku). Robust to the skill not existing yet: skip visibly while the lead writes it.
CDEC="$REPO/.claude/skills/constitution/DECISIONS.md"
CSKILL="$REPO/.claude/skills/constitution/SKILL.md"
if [ ! -f "$CDEC" ]; then
  echo "retention-gate[constitution]: not present yet ($CDEC missing) — skipped"
else
  cn=0; cok=0; CVERIFIED=""
  while IFS= read -r line; do
    id=$(printf '%s' "$line" | awk -F' \\| ' '{print $1}' | sed 's/^| *//; s/ *$//')
    rule=$(printf '%s' "$line" | awk -F' \\| ' '{print $2}')
    cn=$((cn+1))
    if [ -f "$CSKILL" ] && grep -qF -- "$rule" "$CSKILL"; then cok=$((cok+1)); CVERIFIED="${CVERIFIED:+$CVERIFIED, }$id"; else echo "MISSING in constitution/SKILL.md: $id"; fail=1; fi
  done < <(grep '^| C-' "$CDEC")
  # Provenance must point at real sources, not at itself: every `<skill>` §N cited in the Source column
  # must exist as .claude/skills/<skill>/SKILL.md with a "## N." heading (bd memories / prompts are not checked).
  pn=0; pok=0
  while IFS= read -r line; do
    src=$(printf '%s' "$line" | awk -F' \\| ' '{print $3}')
    for skill in $(printf '%s' "$src" | grep -oE '`[a-z0-9-]+` §' | tr -d '`§ ' | sort -u); do
      f="$REPO/.claude/skills/$skill/SKILL.md"; pn=$((pn+1))
      if [ ! -f "$f" ]; then echo "constitution provenance: no such skill $skill"; fail=1; continue; fi
      secs=$(printf '%s' "$src" | grep -oE "\`$skill\` §[0-9]+(, §[0-9]+)*" | grep -oE '§[0-9]+' | tr -d '§' | sort -u)
      miss=0; for n in $secs; do grep -qE "^## $n\." "$f" || { echo "constitution provenance: $skill has no section §$n"; miss=1; }; done
      [ $miss -eq 0 ] && pok=$((pok+1)) || fail=1
    done
  done < <(grep '^| C-' "$CDEC")
  echo "retention-gate[constitution] provenance: $pok/$pn cited skill sections exist"
  echo "retention-gate[constitution] verified ids: ${CVERIFIED:-none}"
  echo "retention-gate[constitution]: $cok/$cn C rules resident in constitution/SKILL.md"
fi
grep -qx 'constitution' "$REPO/agents/peppy/resident.txt" 2>/dev/null || { echo "constitution not in agents/peppy/resident.txt"; fail=1; }

[ "$fail" -eq 0 ] && echo "RESULT: PASS" || { echo "RESULT: FAIL"; exit 1; }
