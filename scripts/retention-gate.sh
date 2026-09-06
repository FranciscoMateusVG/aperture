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
fail=0; n=0; ok=0
while IFS= read -r line; do
  id=$(printf '%s' "$line" | awk -F' \\| ' '{print $1}' | sed 's/^| *//; s/ *$//')
  rule=$(printf '%s' "$line" | awk -F' \\| ' '{print $2}')
  n=$((n+1))
  if grep -qF -- "$rule" "$SKILL"; then ok=$((ok+1)); else echo "MISSING in SKILL.md: $id"; fail=1; fi
done < <(grep '^| DECISION-' "$DEC")
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
echo "retention-gate[standing]: boot $sboot/$sn, precompact $spre/$sn designated standing statements resident"
[ "$fail" -eq 0 ] && echo "RESULT: PASS" || { echo "RESULT: FAIL"; exit 1; }
