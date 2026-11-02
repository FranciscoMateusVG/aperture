# Aperture Dock launcher

The opt-in user LaunchAgent proposal and cutover/rollback runbook live in
[`launchd/README.md`](launchd/README.md). Do not install it until the coordinated
window; the Dock app remains the fallback. Never run two server startup owners.

Run `scripts/install-app.sh` to copy the bundle to `~/Applications/Aperture.app`.
The installer records this checkout's absolute path in the installed bundle.
Run it again if the checkout moves. No compilation or extra runtime is needed.
Previous installations are preserved under
`~/Library/Application Support/Aperture/launcher-backups`, outside Applications.
The icon is copied from v3's `src-tauri/icons/icon.icns`; the installed launcher
does not read anything from v3.

The executable enters `/bin/zsh -lic` before looking up development tools. It
checks hub health before starting anything, waits at most 60 seconds for the
hub, and shows the last hub output in an alert on failure. `scripts/up.sh`
passes the login PATH into the new tmux session even if an older server exists.
Repeated clicks reuse the hub and existing attached client. Simultaneous clicks
are serialized with a PID lock. Logs: `~/Library/Logs/Aperture-launcher.log`.

On this Mac it uses Warp; otherwise it falls back to built-in Terminal. Warp
gets a dedicated `~/.warp/launch_configurations/aperture-v4.yaml`, containing
only the repository directory and a login-shell `tmux attach` command.
It does not overwrite the user's other Warp configurations.
The configuration name is `aperture-v4`, matching the launch URI identifier.
If attachment fails, the alert explicitly says the hub is still running and
provides the manual attach command.
See [Warp URI scheme](https://docs.warp.dev/terminal/more-features/uri-scheme)
and [launch configurations](https://docs.warp.dev/terminal/sessions/launch-configurations).
