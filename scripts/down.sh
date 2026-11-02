#!/usr/bin/env bash
# Stop Aperture: the hub and every agent window. Agent threads and choices are kept.
tmux kill-session -t aperture-v4 2>/dev/null && echo "stopped aperture-v4" || echo "aperture-v4 was not running"
rm -f "$(cd "$(dirname "$0")/.." && pwd)"/data/run/*.sock
