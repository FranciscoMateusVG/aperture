#!/usr/bin/env bash
# retention-gate — §7 standing-rule retention (aperture-trgpo).
# Every DECISION-n rule sentence in orchestrator-core/DECISIONS.md must be present VERBATIM in the
# RESIDENT artifact (orchestrator-core/SKILL.md — the only file the injectors append), and
# orchestrator-core must be in GLaDOS's resident set on both paths. Exit 1 on any breach.
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEC="$REPO/.claude/skills/orchestrator-core/DECISIONS.md"
SKILL="$REPO/.claude/skills/orchestrator-core/SKILL.md"
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
echo "retention-gate: $ok/$n DECISION rules resident in SKILL.md; resident lists OK=$((1-fail))"
[ "$fail" -eq 0 ] && echo "RESULT: PASS" || { echo "RESULT: FAIL"; exit 1; }
