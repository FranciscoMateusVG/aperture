# Boot Verification Harness (aperture-xt16e)

Verifies that Aperture agents actually *boot*: launcher generated, CLI exec'd
with the right argv, Monitor connects to the WS hub, presence-join lands
within SLA. Three layers:

| Layer | What | How |
|-------|------|-----|
| **L1** | Unit — launcher/config generation logic | `just test-rust` (`cargo test` in `src-tauri/`) |
| **L2** | Boot mechanics with **stub** CLIs — argv shape, hello protocol, presence timing, no real model in the loop | `just smoke-boot` (`MODE=l2`, default) |
| **L3** | Real harnesses — actual `claude` / `codex` binaries boot end-to-end | `just smoke-boot l3` (`MODE=l3`) |

## Pieces

- `stubs/claude` — fake claude CLI: records argv+env to
  `$APERTURE_STUB_LOG_DIR/claude-argv.txt`, then execs `stub-hello.mjs` to
  connect to the hub, send the agent hello, and hold the socket (a booted
  agent with a live Monitor). Agent name from `APERTURE_STUB_AGENT_NAME` or
  the launcher's `--name` flag.
- `stubs/codex` — fake codex pane: records argv+env (`codex-argv.txt`) and the
  `--remote unix://<sock>` path (`codex-sock.txt`), sleeps until SIGTERM.
- `stubs/stub-hello.mjs` — WS hello client (retries 500ms × 20 to survive
  hub-not-up-yet; `perMessageDeflate:false`).
- `observer.mjs` — subscriber that asserts expected agents presence-join
  within `--timeout-s`; tails `--hub-log` for `codex_bound` (hub-stderr-only);
  writes time-to-hello per agent to `--out results.json` for SLA drift
  tracking. Exit 0 = all seen, exit 1 = timeout + missing summary.
- `smoke-boot.sh` — orchestrator: free port → hub
  (`APERTURE_HUB_SKIP_REPLAY=1`) → observer → **spawn section** → verdict.
  Green prints a time-to-hello table; failure collects artifacts (hub.log,
  tmux pane captures of the `aperture` session, stub argv logs) into the run
  dir under `/tmp/aperture-boot-harness/`.

## Status: spawn section is BLOCKED

The headless boot entry point does not exist yet (**bead aperture-syepg**,
Peppy). Until it lands, `smoke-boot.sh` exits **3**
(`BLOCKED: headless entry point (aperture-syepg)`) unless you provide
`APERTURE_BOOT_CMD` — an escape hatch that is `eval`'d to perform the spawn.
Stub injection assumes the launcher env knobs `APERTURE_CLAUDE_BIN` /
`APERTURE_CODEX_BIN` / `APERTURE_LAUNCHER_PATH_PREFIX` (parallel worker,
in flight); `smoke-boot.sh` already exports them in `MODE=l2`.

Direct-stub proof (no tmux, no entry point — validates hub+stubs+observer):

```bash
TIMEOUT_S=15 APERTURE_BOOT_CMD='
  APERTURE_STUB_AGENT_NAME=claude-smoke tests/boot-harness/stubs/claude --name claude-smoke &
  APERTURE_STUB_AGENT_NAME=codex-smoke node tests/boot-harness/stubs/stub-hello.mjs --agent codex-smoke &
' tests/boot-harness/smoke-boot.sh
```

## The seven pinned failure modes → coverage map

| # | Failure mode | Covered by |
|---|--------------|-----------|
| 1 | send-keys race under load (tmux window not ready when launcher keys land) | **TODO** — L2 slow-stub variant (stub that delays exec) |
| 2 | `agent_replaced` both orderings (old socket closes before/after new maps) | `mcp-server/test/hub-protocol.test.mjs` (`just test-hub`) |
| 3 | replay-once (unread replay must not double-deliver on reconnect) | hub-protocol tests + future L3 with BEADS in the loop |
| 4 | prompt byte-integrity (system prompt corrupted between manifest and argv) | L1 torture tests (`cargo test`) + L2 argv diff (`claude-argv.txt`) |
| 5 | hub-not-up-yet (agent Monitor connects before hub listens) | `stub-hello.mjs` retry loop; **TODO** smoke-boot ordering variant (spawn agents before hub) |
| 6 | thundering herd (full fleet boots at once) | **TODO** — nightly full-fleet L3 run |
| 7 | codex injection-before-bind (notify delivered before bridge bound a thread) | **TODO** — codex-bridge test with fake app-server; lift the pattern from `mcp-server/scripts/smoke-codex-bridge.mjs` |

## Running each layer

```bash
just test-rust        # L1 — launcher/config unit tests
just test-hub         # hub protocol unit tests (node --test)
just smoke-boot       # L2 — stub CLIs (MODE=l2; exit 3 until aperture-syepg)
just smoke-boot l3    # L3 — real claude/codex binaries
```

Prereqs: `just build-mcp` (hub dist + `ws` in `mcp-server/node_modules`).
