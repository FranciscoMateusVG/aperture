#!/usr/bin/env bash
set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/aperture-just-v4.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

ROOT="$TMP/home/.claude/aperture"
mkdir -p "$ROOT/p1-backend" "$ROOT/p1-broken-marker" "$ROOT/_archived/p1/p1-old"
printf '' > "$ROOT/p1-backend/TEAM"
printf 'preserve-team-seat\n' > "$ROOT/p1-backend/sentinel"
ln -s "$TMP/does-not-exist" "$ROOT/p1-broken-marker/TEAM"
printf 'preserve-invalid-marker-for-fail-closed-loader\n' > "$ROOT/p1-broken-marker/sentinel"
printf 'preserve-archive\n' > "$ROOT/_archived/p1/p1-old/sentinel"

HOME="$TMP/home" just --justfile "$REPO/Justfile" --working-directory "$REPO" setup >/dev/null

test "$(cat "$ROOT/p1-backend/sentinel")" = "preserve-team-seat"
test "$(cat "$ROOT/p1-broken-marker/sentinel")" = "preserve-invalid-marker-for-fail-closed-loader"
test "$(cat "$ROOT/_archived/p1/p1-old/sentinel")" = "preserve-archive"
for manifest in "$REPO"/agents/*/manifest.json; do
  agent=$(basename "$(dirname "$manifest")")
  enabled=$(node -e 'const m=require(process.argv[1]); process.stdout.write(m.enabled === false ? "false" : "true")' "$manifest")
  [ "$enabled" = false ] || test -L "$ROOT/$agent/manifest.json"
done

STATUS=$(just --justfile "$REPO/Justfile" --show status)
grep -Fq 'agents/*/' <<<"$STATUS"
if grep -Fq 'for agent in glados wheatley peppy' <<<"$STATUS"; then
  echo "status still contains a fixed roster" >&2
  exit 1
fi
