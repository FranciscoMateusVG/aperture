# Desktop installer publication (K310B)

Canonical writer: `scripts/publish-desktop-installer.py`. It replaces the inline
Python **copy/fsync/readback block only**, not the `.command` installer, its native
Quit, guards, backups, readback or human confirmation. No daemon or new deployment
framework. Build a single reviewed package and stage its versioned executable
under `/private/tmp` using the existing release process first.

After publication is authorized, invoke once:

```sh
python3 scripts/publish-desktop-installer.py \
  --source '/private/tmp/Atualizar Aperture - COMMIT.command' \
  --sha256 EXPECTED_SHA256
```

Replace `COMMIT` with the lowercase commit hash (7–40 digits; optional `-v2`
suffix). Source must be regular, single-link, owned by this UID, mode `0700`,
at most 256 KiB, with exact SHA. Syntax is checked without executing its bytes.
Destination is always this user's Desktop; no caller destination/deadline knob.

The writer's only child waits for native PID/birth/PPID/UID capture before any
Desktop access. Directory validation and **one** no-follow directory open happen
inside its fixed 60-second budget. All destination operations reuse that dirfd.
File creation is exclusive and initially `0600`; successful publication requires
file fsync, exact readback, `0700`, another file fsync, directory fsync, final
identity/hash readback, orderly close, complete event sequence **and child exit 0**.
Only then does JSON report `ready: true`. It never runs the installer.

On timeout the parent reports the finite current stage, not a guessed OS cause.
It may signal **only its own newly created, unreaped child**, after native
PID/birth/PPID/UID revalidation. No process groups, names, GUI, worker or old helper
can be targets. It allows at most two seconds for reaping; unproven identity or
exit is an explicit pending error. No retry, overwrite, partial-file deletion or
automatic second writer. A leftover `.command` may contain full bytes (and may
already be executable) yet is **not ready** without the successful result. Retain
the JSON result with the existing release receipt. Do not silently retry after
`E_DESTINATION_EXISTS`, timeout or incomplete output.

This bounds the previous indefinite wait and identifies the blocking **stage**;
it does not prove or repair an OS/TCC/filesystem cause. The old PID2688 eventually
returned exit0 before any signal; its sparse log cannot identify which of
directory open/fsync/readback/syntax was delayed. No retroactive syscall claim.

Transient publication/verification processes must never enter `quit_expected`.
Recapture persistent roles after child reaping; any extra GUI descendant still
blocks the real install guard. Publication PASS is not an install, visual/Open,
Claude readiness or provider PASS. Current partial release remains HOLD pending
the single integrated source pin and operator window.

Hermetic native tests (temporary fake Desktop only; no provider or real installer):

```sh
python3 -m unittest discover -s scripts/__tests__ -p test_publish_desktop_installer.py -v
```
