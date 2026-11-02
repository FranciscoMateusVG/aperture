#!/usr/bin/env bash
# Start Aperture: the hub in tmux window "hub", which launches every agent.
#   bash scripts/up.sh      then   tmux attach -t aperture-v4
#   bash scripts/down.sh    stops everything
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SESSION=aperture-v4
if tmux has-session -t "$SESSION" 2>/dev/null; then
  echo "$SESSION already running:  tmux attach -t $SESSION"; exit 0
fi
tmux new-session -d -s "$SESSION" -n hub -c "$ROOT" -e "PATH=$PATH" "mix aperture.up; echo; echo '[hub exited, press enter]'; read"
echo "Starting. Attach with:  tmux attach -t $SESSION   (agents appear as their windows open)"
