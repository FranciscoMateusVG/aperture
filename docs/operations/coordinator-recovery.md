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
