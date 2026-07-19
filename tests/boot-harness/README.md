# Boot Verification Harness (aperture-xt16e)

Verifies that Aperture agents actually *boot*: launcher generated, CLI exec'd
with the right argv, Monitor connects to the WS hub, presence-join lands
within SLA. Three layers:

| Layer | What | How |
|-------|------|-----|
| **L1** | Unit — launcher/config generation logic | `just test-rust` (`cargo test` in `src-tauri/`) |
| **L2** | Boot mechanics with **stub** CLIs — argv shape, hello protocol, presence timing, no real model in the loop | `just smoke-boot` (`MODE=l2`, default) |
| **L3** | Real harnesses — actual `claude` / `codex` binaries boot end-to-end | `just smoke-boot l3` (`MODE=l3`) |

## Status: WIRED to the real headless spawn path

The spawn section drives **`aperture-boot`** (headless entry point,
aperture-syepg, merged in PR #37): `src-tauri/src/bin/aperture-boot.rs` →
`aperture_lib::boot_agent_headless` → `agents::boot_agent_process`. That is
the SAME code the Tauri GUI runs — agent registry load, tmux window creation,
launcher-script generation, Claude kickoff positional, Codex app-server spawn
+ resume gate. `smoke-boot.sh` resolves the bin from
`src-tauri/target/{release,debug}/aperture-boot` and builds it with
`cargo build --bin aperture-boot` if missing. `APERTURE_BOOT_BIN` may point to
an alternate executable for direct-stub debugging; arbitrary shell command
strings are deliberately unsupported.

**Zero-send-keys guarantee**: the harness never calls `tmux send-keys`. Boot
is entirely CLI-arg driven post-#37 — Claude gets a static kickoff positional
baked into its launcher argv; Codex resumes the bridge thread by id. The only
send-keys on the product path is `boot_agent_process`'s own launcher-path
injection at window creation (the product's mechanism, exercised — not
replaced — by every run).

## How a run works

1. **Guards** — resolve the real node binary (the `node` on PATH may be a
   volta shim that doesn't forward SIGTERM; killing the shim orphans the
   actual listener), rebuild `mcp-server/dist` if missing or older than any
   `src/*.ts` (stale-dist false-failure guard).
2. **Registry** — `APERTURE_AGENTS_DIR` points at `registry/` (stub agents
   `claude-smoke` model `opus`, `codex-smoke` model `codex/gpt-5`). The real
   load path (`agent_loader.rs`) requires, per agent:
   `manifest.json` (serde-required fields: `name`, `model`, `window`, `role`;
   `kind`/`enabled`/`emoji` optional) + `prompt.md`; `skills/<s>/SKILL.md`
   is optional and exercised via `inject_skills`.
3. **Isolated tmux session** — `aperture-smoke-$$`, created by the harness
   (`tmux_create_window` requires the session to already exist), exported as
   `APERTURE_TMUX_SESSION`, killed on exit. The production `aperture` session
   is never touched. Stub env (log dir, hub port, real-node path) is planted
   in the tmux *session* environment — panes don't inherit the script's
   exports.
4. **Hub + observer** — free port, `APERTURE_HUB_SKIP_REPLAY=1`. In l2 the
   hub's codex-bridge discovers `codex-smoke` and endlessly fails to bind
   against the non-JSON-RPC stub app-server — **expected noise**; nothing
   asserts on bridge events in l2. The l2 hub gets an isolated
   `APERTURE_RUN_DIR` because the bridge clears `<run>/<agent>.thread-id` on
   every failed connect, which would race the pre-seed below.
5. **Spawn** — `aperture-boot --agent claude-smoke --agent codex-smoke [...]`
   with `APERTURE_CLAUDE_BIN` / `APERTURE_CODEX_BIN` /
   `APERTURE_LAUNCHER_PATH_PREFIX` pointing at `stubs/` in l2.
6. **Verdict** — observer asserts every claude join within `TIMEOUT_S`;
   `results.json` records per-agent time-to-hello. l2 then asserts the codex
   pane artifacts (below).

### The l2 codex pre-seed (and why)

The codex pane launcher (PR #37 shape) waits ≤10s for the app-server sock,
then ≤60s for the thread-id file (`~/.aperture/run/<agent>.sock` with `.sock`
→ `.thread-id`, per `codex_appserver::socket_path` +
`launcher::build_codex_launcher`), then execs
`codex resume "$THREAD_ID" --remote unix://<sock>`. In l2 there is no real
bridge to publish a thread id, so the harness seeds
`~/.aperture/run/codex-smoke.thread-id` with `l2-stub-thread`. Because
`boot_agent_process` clears that exact file as stale at boot
(`agents.rs`), the harness re-seeds right after `aperture-boot` returns —
well inside the pane's 10s dead-sock wait. The l2 codex assertion then proves
the pane binary was exec'd with argv containing `resume`, `l2-stub-thread`,
`--remote`, and a `unix://…` sock — i.e. the resume-gate launcher shape
flowed through the REAL spawn path. Do **not** pre-seed in l3: the real
bridge owns the file.

## FLEET knob (failure mode #6 — thundering herd)

`FLEET=<n>` (or `just smoke-boot-fleet <n> [mode]`, default n=7) copies
`registry/` into a temp overlay under the run dir, generates `fleet-1..n`
claude manifests there, and boots **all** agents (`claude-smoke`,
`codex-smoke`, n fleets) in one `aperture-boot` invocation. The observer
expects every claude agent (n+1 joins in l2 — codex stays pane-artifact
verified; n+2 in l3) and `results.json` carries per-agent time-to-hello for
SLA drift tracking.

## Pieces

- `registry/` — stub agent registry (see load-path requirements above).
- `stubs/claude` — fake claude CLI: records argv+env to
  `$APERTURE_STUB_LOG_DIR/claude-argv.txt`, then execs `stub-hello.mjs` to
  connect to the hub, send the agent hello, and hold the socket (a booted
  agent with a live Monitor). Agent name from `APERTURE_STUB_AGENT_NAME` or
  the launcher's `--name` flag.
- `stubs/codex` — fake codex binary, invoked TWICE per run through the real
  path: as the supervised app-server child (`codex app-server --listen …`)
  and as the tmux pane (`codex resume <id> --remote …`). Records argv
  (`codex-argv.txt`) and the `--remote unix://<sock>` path (`codex-sock.txt`),
  sleeps until SIGTERM. **Deliberately silent toward the hub** — in the real
  system the codex pane never talks to the hub, so a pane-side hello would be
  a fake signal.
- `stubs/stub-hello.mjs` — WS hello client (retries 500ms × 20 to survive
  hub-not-up-yet; `perMessageDeflate:false`).
- `observer.mjs` — subscriber that asserts expected agents presence-join
  within `--timeout-s`; tails `--hub-log` for `codex_bound` (hub-stderr-only);
  writes time-to-hello per agent to `--out results.json` for SLA drift
  tracking. Exit 0 = all seen, exit 1 = timeout + missing summary.
- `smoke-boot.sh` — orchestrator (flow above). Green prints a time-to-hello
  table; failure collects artifacts (hub.log, aperture-boot.log, tmux pane
  captures of the smoke session, stub argv logs) into the run dir under
  `/tmp/aperture-boot-harness/`.

## The seven pinned failure modes → coverage map

| # | Failure mode | Covered by |
|---|--------------|-----------|
| 1 | send-keys race under load (tmux window not ready when launcher keys land) | **ELIMINATED by design** — post-#37 kickoff is CLI-arg driven (Claude positional / Codex resume gate), no harness or product kick via send-keys. Residual send-keys surface = the launcher-path injection at window creation, covered implicitly by every l2 run (the pane demonstrably ran the launcher). |
| 2 | `agent_replaced` both orderings (old socket closes before/after new maps) | `mcp-server/test/hub-protocol.test.mjs` (`just test-mcp`) |
| 3 | replay-once (unread replay must not double-deliver on reconnect) | `mcp-server/test/hub-replay.test.mjs` (`just test-mcp`) + future L3 with BEADS in the loop |
| 4 | prompt byte-integrity (system prompt corrupted between manifest and argv) | L1 torture tests (`cargo test`) + L2 argv diff (`claude-argv.txt`) |
| 5 | hub-not-up-yet (agent Monitor connects before hub listens) | `stub-hello.mjs` retry loop; **TODO** smoke-boot ordering variant (spawn agents before hub) |
| 6 | thundering herd (full fleet boots at once) | **FLEET knob** — `just smoke-boot-fleet` (L2 today, nightly full-fleet L3 later) |
| 7 | codex injection-before-bind (notify delivered before bridge bound a thread) | `mcp-server/test/codex-bind-order.test.mjs` (`just test-mcp`) |

## Running each layer

```bash
just test-rust             # L1 — launcher/config unit tests
just test-mcp              # hub/replay/bind suites (always rebuilds dist first)
just smoke-boot            # L2 — stub CLIs through the real spawn path
just smoke-boot-fleet 7    # L2 thundering herd (FLEET=7)
just smoke-boot l3         # L3 — real claude/codex binaries
```

Prereqs: `pnpm install` in `mcp-server/` (the smoke auto-rebuilds a stale
dist); tmux (the harness starts a server if none is running); cargo for the
aperture-boot bin.
