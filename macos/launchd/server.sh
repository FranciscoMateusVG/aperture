#!/bin/bash
# Template: install.sh renders only non-secret paths/user metadata.
# Installed at ~/Library/Application Support/Aperture/launchd/server.sh.
set -euo pipefail
umask 077

export HOME=__APERTURE_HOME__
export USER=__APERTURE_USER__ LOGNAME=__APERTURE_USER__ SHELL=/bin/zsh
export VOLTA_HOME="$HOME/.volta"
export PATH="$VOLTA_HOME/bin:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export BEADS_DIR="$HOME/.aperture/.beads"
export LANG=en_US.UTF-8 LC_CTYPE=en_US.UTF-8
export MIX_ENV=dev TERM=xterm-256color
export TMPDIR="$(/usr/bin/getconf DARWIN_USER_TEMP_DIR)"
repo=__APERTURE_REPO__
expected_uid=__APERTURE_UID__
session=aperture-v4

fail() { printf '%s\n' "Aperture: $1" >&2; exit 75; }
[[ "$(/usr/bin/id -u)" = "$expected_uid" ]] || fail "wrong user"
[[ -f "$repo/mix.exs" ]] || fail "repository missing"
cd "$repo"
for tool in mix elixir erl node tmux bd claude codex; do
  command -v "$tool" >/dev/null || fail "required tool missing: $tool"
done

# Native credentials stay in HOME/keychain. Never source rc/dotenv files or
# print env/auth status. SSH_AUTH_SOCK is a socket locator, not a credential.
sock="$(/bin/launchctl getenv SSH_AUTH_SOCK 2>/dev/null || true)"
if [[ -n "$sock" && -S "$sock" ]]; then export SSH_AUTH_SOCK="$sock"; fi
if [[ "${1:-}" = --check && $# = 1 ]]; then
  printf '%s\n' "Aperture: environment/tool-path preflight OK (auth not checked)"
  exit 0
fi
[[ $# = 0 ]] || fail "unsupported argument"

# Refuse a competing server. Do not adopt/kill it or claim launchd owns it.
# The eventual exclusive bind is authoritative if a competing launch races us.
if /usr/sbin/lsof -nP -iTCP:4518 -sTCP:LISTEN -t >/dev/null 2>&1; then
  fail "port 4518 occupied; startup refused"
fi

# Preserve running agent windows and thread records. An unrelated tmux server
# may have a stale PATH: set only this session's non-secret environment.
if ! tmux has-session -t "$session" 2>/dev/null; then
  tmux new-session -d -s "$session" -n console -c "$repo" -e "PATH=$PATH"
fi
for name in HOME USER LOGNAME SHELL PATH VOLTA_HOME BEADS_DIR LANG LC_CTYPE; do
  tmux set-environment -t "$session" "$name" "${!name}"
done
if [[ -n "${SSH_AUTH_SOCK:-}" ]]; then
  tmux set-environment -t "$session" SSH_AUTH_SOCK "$SSH_AUTH_SOCK"
fi

printf '%s\n' "$(/bin/date -u +%FT%TZ) Aperture: starting foreground hub"
# Never scripts/up.sh: launchd must supervise the hub, not a detached wrapper.
exec /opt/homebrew/bin/mix aperture.up
