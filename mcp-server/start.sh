#!/usr/bin/env bash
# Aperture MCP server launcher.
# AGENT_NAME must be set in the environment before starting Claude Code.
# Example: AGENT_NAME=wheatley claude --name wheatley
set -euo pipefail

if [ -z "${AGENT_NAME:-}" ]; then
  echo "ERROR: AGENT_NAME environment variable is not set." >&2
  echo "Launch Claude Code with: AGENT_NAME=<agent> claude ..." >&2
  exit 1
fi

exec node "$(dirname "$0")/dist/index.js"
