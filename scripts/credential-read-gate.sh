#!/usr/bin/env bash
# credential-read-gate — no skill or prompt may instruct a model-visible credential read.
# Standing rule: credential-drawer-plaintext-read-ban (Cipher, 2026-08-28). Regression for the
# aperture-3kavd HOLD: verify-user-path §"Test-walker credentials" told agents to call
# mempalace_get_drawer on the peppy/secrets drawers, which puts passwords into transcripts.
# Usage: scripts/credential-read-gate.sh   (exit 1 on any hit)
set -uo pipefail
cd "$(dirname "$0")/.."
fail=0
# Instruction-shaped patterns only: a tool call or shell read that returns drawer contents to the model.
PATTERNS='mempalace_get_drawer\(|mempalace_search\([^)]*secret|cat [^|]*peppy/secrets|drawer_peppy_secrets_[0-9a-f]+"\)'
while IFS= read -r f; do
  if hits=$(grep -nE "$PATTERNS" "$f"); then
    echo "MODEL-VISIBLE CREDENTIAL READ INSTRUCTION in $f:"; echo "$hits" | sed 's/^/  /'; fail=1
  fi
done < <(find .claude/skills -name SKILL.md; ls prompts/*.md 2>/dev/null)
# The specific stale instruction must stay gone, and the replacement contract must be present.
S=.claude/skills/verify-user-path/SKILL.md
grep -q 'NON-MODEL delivery only' "$S" || { echo "$S: non-model delivery contract paragraph missing"; fail=1; }
grep -q 'STOP and ask' "$S" || { echo "$S: stop-and-ask clause missing"; fail=1; }
[ $fail -eq 0 ] && echo "credential-read-gate: PASS (no model-visible credential read instructions in skills/prompts)"
exit $fail
