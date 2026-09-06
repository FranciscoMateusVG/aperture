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
# Synthetic-secret fixtures must never carry a contiguous detector-shaped literal at rest (GitGuardian
# 37036434/37036433, aperture-3kavd): they are stored as fragments and re-joined at test time.
FIXTURE_PAT='BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY|sk_(live|test)_|sk-[A-Za-z0-9]{10,}|ghp_[A-Za-z0-9]{10,}|xox[bap]-[0-9]{6,}|AKIA[0-9A-Z]{8,}|password=|BEADS_DOLT_PASSWORD='
for f in mcp-server/test/fixtures/*.json; do
  if hits=$(grep -nE "$FIXTURE_PAT" "$f"); then
    echo "CONTIGUOUS DETECTOR-SHAPED LITERAL AT REST in $f (store as text_parts/marker_parts):"; echo "$hits" | cut -c1-40 | sed 's/^/  /'; fail=1
  fi
done
[ $fail -eq 0 ] && echo "credential-read-gate: PASS (no model-visible credential read instructions in skills/prompts; fixtures fragmented at rest)"
exit $fail
