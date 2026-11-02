# Aperture user LaunchAgent

Task `aperture-9ni3r`. This is an opt-in replacement for the **server startup**
part of the Dock app. The PWA is separate. Nothing here runs at checkout time.

**Do not install or activate until GLaDOS and Francisco open the cutover window,
after the Phoenix/chat work has merged. The current server hosts the agents
performing this work. Keep an independent Terminal/Warp window open for recovery.**

## What is supervised

`launchd → env -i → bash --noprofile --norc → exec mix aperture.up`.
The foreground BEAM process owns `127.0.0.1:4518`. `KeepAlive=true` restarts it
after exit, with a 30-second throttle. Do **not** run `scripts/up.sh` under
launchd: that script detaches into tmux, leaving launchd supervising the wrong
process. This is not a health watchdog; a live but wedged process is not restarted.

The GUI-user agent starts at login, not before login. Logout, sleep and power-off
are not uptime guarantees. It runs without sudo and never exposes the hub beyond
loopback. `AbandonProcessGroup` is not enabled: keep launchd's cleanup of the hub's
process group. The existing tmux server is a separate process group; agents remain
in tmux and reconnect when only the hub restarts. Verify that property again in
the controlled crash test, especially after a cold login creates a new tmux server.

## Files and explicit environment

| File | Installed location |
| --- | --- |
| `com.aperture.server.plist` | `~/Library/LaunchAgents/com.aperture.server.plist` (600) |
| `server.sh` | `~/Library/Application Support/Aperture/launchd/server.sh` (700) |
| stdout/stderr | `~/Library/Logs/Aperture/server.{stdout,stderr}.log` (600) |

The checked-in files are **templates**, not directly installable with `cp`.
`install.sh` renders the login user's HOME/name/UID and the explicitly selected
main checkout. Shell quoting and plist serialization handle spaces and XML
characters. It requires macOS's `/usr/bin/python3`, `plutil` and `bash`; it downloads
nothing. The installed script is a reviewed copy, not a symlink into a worktree.

The installer refuses root, a non-main target checkout, symlinks in destination
paths and files owned by another user. It never bootstraps/stops/enables/disables
anything, and refuses install/uninstall if the job is loaded. Previous definition
and script copies are preserved in `launchd/backups/`. Logs and all application
data are preserved. Installation alone makes the definition eligible at the next
GUI login, so even file installation belongs inside the approved window.

Environment reconstructed without any rc/dotenv file:

- HOME, USER, LOGNAME, SHELL; `VOLTA_HOME=$HOME/.volta`.
- PATH: Volta, `/opt/homebrew/bin`, `/opt/homebrew/sbin`, `/usr/local/bin`, then
  macOS system directories. Stable Homebrew links, no version-specific Cask paths.
- `BEADS_DIR=$HOME/.aperture/.beads`, `MIX_ENV=dev` (the existing server mode),
  `LANG=en_US.UTF-8`, `LC_CTYPE=en_US.UTF-8`, `TERM=xterm-256color`.
- TMPDIR from `getconf DARWIN_USER_TEMP_DIR`; SSH_AUTH_SOCK from the user's
  launchd environment only if it points to an existing socket.
- Tool presence: mix, elixir, erl, node, tmux, bd, claude, codex.

The inspected `.zshenv` exports PATH/VOLTA_HOME/BEADS_DIR; `.zshrc` adds PATH and
Android-specific directories/variables. Android variables are not used by the
hub. No authentication env exports were identified or supplied by the Dock app.
Claude and Codex retain their native keychain/HOME stores; we neither read/copy
their credential values nor invent API-key variables. Keychain unlock/access
after login is an acceptance check, not something `--check` proves.

Only non-secret variables are copied into **this tmux session's** environment so
new agent windows do not inherit an unrelated tmux server's stale PATH. Existing
agent processes are not restarted. Never put auth secrets in tmux commands,
generated scripts, plist, git, task notes or logs; no `launchctl setenv` secrets.

The wrapper refuses to start if anything already listens on 4518, exiting75.
It never kills/adopts that listener. Exclusive socket bind remains authoritative
if two launchers race. Do not click Dock/run `up.sh` during the cutover or use them
as competing startup mechanisms while the LaunchAgent is enabled.

## Safe checks now (no installation, no running server changes)

From this checkout/worktree, with the real main checkout explicitly selected:

```sh
bash -n macos/launchd/server.sh
bash -n macos/launchd/install.sh
plutil -lint macos/launchd/com.aperture.server.plist
bash macos/launchd/install.sh check --repo "$HOME/projects/aperture-v4"
```

`check` renders in memory, runs real bash/plutil validation and executes only
the wrapper's `--check` path under `env -i`. It does not create files, start the
hub, modify tmux, connect/login to providers, or prove auth/KeepAlive. It must fail
if `--repo` is missing, relative, invalid or on the task branch instead of main.
Do not run `server.sh` directly: the checked-in placeholders are not configuration.

## Single approved cutover window

These are **operator-run commands**, not instructions to execute during review.
No control step should depend on an MCP call served by the hub being stopped.
The initial switch plus the requested crash test produce two supervised starts;
there is no separate pre-Phoenix installation/restart.

### 1. Prepare from an independent login terminal

After GLaDOS has merged this task **and** the Phoenix/chat changes into main:

```sh
REPO="$HOME/projects/aperture-v4"
LABEL=com.aperture.server
DOMAIN="gui/$(id -u)"
SERVICE="$DOMAIN/$LABEL"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
cd "$REPO"
git branch --show-current                  # must be main
git log -1 --oneline                       # confirm the agreed merged version
bash macos/launchd/install.sh check --repo "$REPO"
tmux list-panes -s -t aperture-v4 -F '#{window_name} pid=#{pane_pid} cmd=#{pane_current_command}'
curl -fsS --max-time 3 http://127.0.0.1:4518/health
```

Checkpoint busy agents, confirm the new `mix aperture.up` still binds4518 and
starts the agents, and record thread/window identities without credential values.
Do not open the window until dependencies/build prerequisites for the merged app
are ready. `/health` and `/inbox` must remain compatible with the new app. No
application writes or test suite should use the production SQLite database.

**Only now, inside the agreed window**, install the reviewed copies. This step
leaves the existing manual hub untouched and does not load the job:

```sh
bash macos/launchd/install.sh install --repo "$REPO"
```

### 2. Stop only the identified manual hub, then bootstrap

Run this block only after confirming the listener belongs to the current manual
hub, not an already supervised service. It checks a single BEAM listener and
will not escalate to SIGKILL if graceful stop times out:

```sh
(
  set -eu
  if launchctl print "$SERVICE" >/dev/null 2>&1; then
    echo 'STOP: LaunchAgent already loaded; inspect ownership.' >&2; exit 1
  fi
  old_pid="$(/usr/sbin/lsof -nP -iTCP:4518 -sTCP:LISTEN -t)"
  case "$old_pid" in ''|*[!0-9]*) echo 'STOP: need exactly one listener PID.' >&2; exit 1;; esac
  comm="$(ps -p "$old_pid" -o comm=)"
  case "$comm" in */beam.smp) ;; *) echo 'STOP: unexpected listener.' >&2; exit 1;; esac
  ps -p "$old_pid" -o pid=,ppid=,pgid=,comm=
  # GLaDOS/Francisco have confirmed this exact PID in this window.
  kill -TERM "$old_pid"
  for attempt in $(seq 1 30); do
    if ! /usr/sbin/lsof -nP -iTCP:4518 -sTCP:LISTEN -t >/dev/null 2>&1; then break; fi
    sleep 1
  done
  if /usr/sbin/lsof -nP -iTCP:4518 -sTCP:LISTEN -t >/dev/null 2>&1; then
    echo 'STOP: listener remains; do not bootstrap or force-kill.' >&2; exit 1
  fi
  launchctl enable "$SERVICE"
  launchctl bootstrap "$DOMAIN" "$PLIST"
)
```

Do not run `scripts/down.sh`, `tmux kill-session` or remove sockets/WAL files.
If the block fails, stop and investigate; do not launch a second hub or rerun the
manual-hub kill block against the new supervisor's listener.

### 3. Verify the actual service and reconnection

```sh
(
  for attempt in $(seq 1 120); do
    if curl -fsS --max-time 1 http://127.0.0.1:4518/health >/dev/null; then
      echo 'HTTP health OK'; exit 0
    fi
    sleep 1
  done
  echo 'Health deadline exceeded: inspect private logs; no automatic restart.' >&2
  exit 1
)
launchctl print "$SERVICE" | grep -E '^[[:space:]]*(state|pid|last exit code) ='
/usr/sbin/lsof -nP -iTCP:4518 -sTCP:LISTEN -Fpcn
tmux list-panes -s -t aperture-v4 -F '#{window_name} pid=#{pane_pid} cmd=#{pane_current_command}'
```

Compare launchd's PID to the listener. Check the actual PWA, inbox and terminal
agents. `/inbox` is a **WebSocket**, so a plain HTTP GET/200 is not its acceptance
test. Send a marker via the existing Aperture `send_message` tool in each
direction between agents and operator, observe delivery/ack through the real
inbox, confirm BEADS with `list_tasks` (read-only), and confirm unchanged agent
thread IDs and native Claude/Codex login. Do not start parallel smoke servers
against the real data directory. Report HTTP healthy separately from agents
reconnected, task tools working, and `visually verified: yes/no`.

### 4. Controlled KeepAlive test (still requires the agreed moment)

After step3 passes and agents are checkpointed, record the listener PID. This
deliberately kills **only the service launchd owns**, never an unverified port PID:

```sh
before="$(/usr/sbin/lsof -nP -iTCP:4518 -sTCP:LISTEN -t)"
launchctl kill SIGKILL "$SERVICE"
```

Repeat step3 with its deadline, allowing the 30-second throttle. Require a
different listener PID, running service, the same tmux agent processes/threads,
and new real inbox round-trips. If any fail, report before attempting remediation.
The job is restart-on-exit, not just restart-on-failure.

Logout/login proof is a separate operator-controlled checkpoint: Francisco logs
out/in when no agent work is at risk, opens neither Dock nor terminal startup,
then confirms the PWA and native auth work. Do not initiate logout from an agent.
Until witnessed, record `login restart: not verified`; do not claim full acceptance.

## Normal operation (coordinate interruption first)

Use the variables from step1 in an independent terminal:

```sh
# Graceful restart, KeepAlive brings it back:
launchctl kill SIGTERM "$SERVICE"

# Stop persistently, including later logins:
launchctl disable "$SERVICE"
launchctl bootout "$SERVICE"

# Resume after bootout:
launchctl enable "$SERVICE"
launchctl bootstrap "$DOMAIN" "$PLIST"
```

`kickstart -k` kills the running instance; it is not the default maintenance
method. `disable` alone is not a stop; `bootout` without disabling does not mean
the plist is disabled at next login. Never bootout the entire `gui/UID` domain.
Before replacing installed files, disable/bootout and confirm the job is unloaded.

Logs are private append-only files. launchd does not rotate them. During planned
maintenance, with the service stopped, archive files above10MiB and retain the
last five archives according to an operator-approved retention action. There is
no automatic size cap while the process runs; no second scheduler/watchdog is
installed by this task. Do not paste unfiltered application logs into messages.

## Rollback to the existing Dock app

Do not delete `~/Applications/Aperture.app`, native auth stores or any Aperture
data. After arranging the rollback window, disable/bootout the LaunchAgent with
the commands above and verify there is no listener4518.

If there is **no** tmux session, the old path works unchanged:

```sh
open "$HOME/Applications/Aperture.app"
```

If `aperture-v4` still exists, **`scripts/up.sh` only returns “already running”**;
it does not recover a dead hub. From the independent login terminal:

```sh
tmux list-panes -s -t aperture-v4 -F '#{window_name} pane=#{pane_id} cmd=#{pane_current_command}'
tmux set-environment -t aperture-v4 PATH "$PATH"
# If the old hub.0 pane exists, and has been confirmed to contain ONLY the
# stopped hub's waiting shell (never an agent or an active server):
tmux respawn-pane -k -t aperture-v4:hub.0 -c "$REPO" '/bin/zsh -lic "exec mix aperture.up"'
# OTHERWISE, if there is no hub window, use this INSTEAD, not both commands:
# tmux new-window -d -t aperture-v4 -n hub -c "$REPO" '/bin/zsh -lic "exec mix aperture.up"'
```

Wait for health and agent reconnection, then `open "$HOME/Applications/Aperture.app"`.
The Dock launcher sees the healthy server and only opens/attaches the terminal.
Do not respawn any pane until its identity and inactive state are confirmed.

The installed job can remain disabled for easy return. To remove only its two
installed files, after bootout (never while loaded):

```sh
bash "$REPO/macos/launchd/install.sh" uninstall
```

Uninstall archives the files first and preserves logs, database/WAL, tmux,
threads, Dock app and credentials. It does not reset launchd's disabled override;
a subsequent reinstall still needs `launchctl enable` in an approved window.

## Evidence and references

Before installation, observed on macOS15.7.4: all tools resolved with `env -i`;
Volta Node24.15.0, Mix1.20.4/OTP29, tmux3.6a, Claude2.1.281, Codex0.160.0,
bd1.0.2. This is a baseline, not a pin or an authenticated-start proof.

- Native `man launchd.plist` / `man launchctl` on the target Mac.
- [Apple: Creating Launch Daemons and Agents](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html).
- Existing Dock launcher: commit01379b8; `lib/mix/tasks/aperture.up.ex`,
  `lib/aperture/launcher.ex`, `lib/aperture/tmux.ex` for the real foreground/reconnect path.
- Recall: `aperture-gui-launch-path-starvation-aperture-3x136-fixed`.

Installation, kill recovery, inbox reconnect and logout/login remain **unverified**
until the authorized window. Preparing/merging these files is not a deploy.
