# Local coordinator recovery

This applies to standing coordinator seats, not team retirement.

* Start/attach preserves the private CODEX_HOME and never copies or inspects
  authentication from the global Codex home. Existing temporary homes remain
  supported. An explicitly configured `~/.aperture/codex/<seat>` takes precedence.
* Before a fresh app-server, the manifest prompt and resident skills are rebuilt
  under `~/.aperture/prompts/<seat>.md` (0600); only the instructions path in the
  existing TOML is changed. Provider, approval, trust and unknown keys survive.
  No unbounded memory-index shell is run; current memory is retrieved via BEADS.
* Adoption/attach does **not** rewrite a running app-server's configuration. An
  absent temporary prompt is restored from the durable copy (or canonical sources
  if neither exists). The durable configuration takes effect at the next fresh
  spawn. The compatibility copy does not promise permanent retention of `/tmp`.
* Stop/Restart now use current exact tmux panes and the registered native Codex
  socket/process binding, not the cached UI running flag. Stop is a **force stop**,
  not a promise to save an in-flight provider turn, shell task or build. The
  explicit operator action freezes the native PID/birth/UID ancestry closure,
  records it privately, then terminates individual identities and verifies Gone.
  Process groups and process-name matches never authorize a signal. A closure
  containing the controller itself is refused instead of taking the server down.
* Any uncertain observation/effect remains Unknown in `run/coordinator-stops` and
  blocks automatic retries. Successful stops retain their receipts. Completed
  Codex incarnations move intact to `run/coordinator-retired` only after Gone and
  fixed-endpoint cleanup; the conversation thread file is retained for resume.
* Native identity-check-to-signal and pathname-recheck-to-use are not atomic
  against a hostile same-UID process. This is the existing personal-local threat
  boundary, not a kernel isolation or global remote-effect-completion claim.

## Delivery

Build server, boot and team-control from the same reviewed commit using the
existing `just` recipe, with the matching versioned UI. Do not launch the server
as a child of an agent being stopped, and do not terminate the existing server
while it owns the current control session without an explicit handoff. No
production merge or team retirement is implied by this local repair.

## Recovering after Unknown

The last stop is `~/.aperture/run/coordinator-stops/<seat>.json`. A later explicit
Start/Stop/Restart may reconcile it only if the full frozen closure was recorded,
every exact identity was kill-dispatched, and every recorded PID/birth is now
Gone (or the PID has been recycled). It preserves the original Unknown receipt
under a unique `*-unknown-*.json` name and records the completed observation;
this does not replay signals. A same/live, unreadable or incomplete closure
still refuses. In that case inspect the exact recorded identities and live
native ancestry; do not delete the receipt or infer that an empty pane is Gone.
If anything remains uncertain, preserve it for a specifically scoped recovery.

Before the first operator Stop of a manually restored pane, normalize only its
verified exact tmux window ID to the registered seat name (for example the
manually named `glados-` window). No prefix match is used to broaden signal
ownership. A concurrently appearing foreign pane is left alone, reports Unknown,
and does not cause an already-dead old worker to be displayed as running.

TOML updates preserve values, including trust/approval/provider/unknown keys, but
serialization may reorder keys and drop comments. Authentication is untouched.
