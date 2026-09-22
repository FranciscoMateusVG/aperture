# Open an existing managed terminal

The GUI sends only team, seat and expected owner generation to `team_open_seat`.
The command requires the current Active Codex owner with an exact observed tuple,
thread and process identity. Under team then seat locks it derives the runtime
CODEX_HOME and socket from native state, checks private non-symlink directory
components, checks the socket's kernel peer PID (`LOCAL_PEERPID`), rechecks process
birth/UID, and derives the executable from that process rather than PATH.

Open creates or selects only a tmux TUI client. The existing paired aperture-boot
binary has a separate `--attach-managed` entrypoint that never enters legacy
boot. It reacquires the locks and verifies the exact owner hash, current socket,
client PID/birth and window before exec of `codex resume EXACT_THREAD --remote
unix://EXACT_SOCKET`. Here resume attaches the existing remote thread; it does not
create a replacement, select newest, start another app-server or send a kickoff.

A private client receipt enables reuse of the exact window. A live mismatched
client blocks rather than being killed. Closing this client does not stop the
worker. Tmux subprocesses use the existing bounded native subprocess runner;
unknown command outcomes have no automatic retry. The helper is paired from the
retained release tree at the compile-time Cargo directory, not an arbitrary
binary supplied by a caller. That tree must not be removed while installed.

The response has exactly team, seat, generation and window_id. A shared fixture
is asserted against Rust serialization and consumed by the UI parser. Open
selects the resulting tmux window, but source tests do not prove installed
terminal visibility or successful remote attachment. The command does not claim
provider attestation or eliminate the accepted same-UID check-to-exec residual.

## Tests and remaining installed witness

Unit/component tests cover owner/model/generation/process/thread negatives,
selector and response rejection, double click, existing-client identity reuse,
private-path symlink rejection, a real disposable Unix socket's peer PID,
and the exact TUI argv. No real Codex or managed owner is used by these tests.

After a paired release and authorized window, Open twice must select the same
client on the exact owner thread; closing that client must leave the worker
identity unchanged. Changed owner/socket/client identity must refuse attachment.
This witness is NOT_RUN until explicitly recorded; no new worker or mission is
started by Open, and Claude remains outside this Codex-only command.
