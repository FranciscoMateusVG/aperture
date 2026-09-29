# b2ajt F0 — final isolated Node baseline and acceptance addendum

Supersedes only the pending runner/status statements in `2026-09-27-b2ajt-f0-source-baseline.md` at docs commit `e5aa8db20cebe60d9745787134321411484d18b7`. Earlier runner-blocked logs remain unchanged. Product source remains exactly `abeb9aedadfa10cfc6d2c42bdaeb0d74ee112c24`; this task has changed documentation only.

## Authorized dependency preparation and result

Root authorization: aperture-wisp-o6fr6y. Existing executable `/opt/homebrew/bin/pnpm`, version **10.33.0**, compatible with lockfile version **9.0**. Process-local PATH prefixed with `/Users/franciscomateus/.volta/tools/image/node/22.22.3/bin`; no global tool configuration changed.

In task worktree `/Users/franciscomateus/projects/aperture-worktrees/aperture-b2ajt-web-local`:

```sh
PATH="/Users/franciscomateus/.volta/tools/image/node/22.22.3/bin:$PATH" pnpm install --offline --frozen-lockfile --ignore-scripts
PATH="/Users/franciscomateus/.volta/tools/image/node/22.22.3/bin:$PATH" /Users/franciscomateus/.volta/tools/image/node/22.22.3/bin/node --test tests/*.test.mjs
```

Install: **exit 0**, reused 21 packages, downloaded 0, no lifecycle scripts. Test: **exit 0, 179 passed, 0 failed, 0 skipped**, 3838.953042 ms. Exactly one install and one subsequent suite execution under this authorization.

Before and after hashes match:

- `pnpm-lock.yaml`: `be52c5a02f99456d2254c058ac88574a02da3b55bad1b458455ba6beac0e47aa`.
- `package.json`: `576b42ac59190b6e727b10c4c626b3e17408c242f125590582c61d5843af8617`.
- `git diff --exit-code -- package.json pnpm-lock.yaml src-tauri/Cargo.lock`: exit 0, empty. Worktree clean before writing this addendum.

Unique evidence under `tmp/f0-20260927T1400/` (retained in task worktree):

| Log | SHA-256 |
| --- | --- |
| `pnpm-offline.log` | `358d7812272e0942a791d472676e59100db798a51fc85b01b411199c0469ffb4` |
| `node-baseline-offline-deps.log` | `28911d3c85d00cdb3b1c5775d0b0dd6e217afccff07aff811ec3548112dd3124` |

Original Rust baseline remains **550 passed / 0 failed / 9 ignored**, exit 0, no rerun. Initial Node shim exit 126 and missing-Vite suite failures are **RUNNER_BLOCKED**, not product assertion failures; both original logs and exit files are retained.

## QA/root acceptance refinements

QA aperture-wisp-wy4csq reviewed source SHA `abeb9aedadfa10cfc6d2c42bdaeb0d74ee112c24`: PASS only static correspondence, HOLD before F1. Receipt sent naming that SHA. Root ybd4ju/j88rzq confirmed the narrower web-bootstrap policy and wrapper-only Open seam described in the source map. Cipher still owns final HTTP denial code/status and auth contract; F1 still needs root GO.

- Authenticate session before `main.ts` tmux creation/polling. Never reuse `lib.rs::run()` as server startup: it initializes bd/dolt and daemons.
- HTTP tests cover 20 routes explicitly. Existing injected DTO/component tests are not HTTP integration tests; service-call counts are not coverage.
- Native team selector fields remain snake_case inside the input DTO. Translate camelCase Tauri outer arguments deliberately; preserve TeamError/non-2xx and no mutation retry.
- Existing durable launch diagnostics are **Claude-specific**. Future two-Codex lifecycle needs real Codex fault oracles; do not relabel a Codex exit as Claude gate_error without evidence. Retain inert Claude diagnostic tests as a separate contract.
- Real-bd test proves only isolated DB/assignee behavior, not managed lifecycle. MCP readiness tests are outside these two baseline commands. Boot-harness pre-seeded thread is not real managed READY.
- Controller-lock integration must explicitly cover fallback before claiming it lock-aware. An otherwise unchanged Tauri application is not restart/survival evidence.

## Boundary of this delivery

F0 requested source map, file order and isolated existing baselines are now supplied. This is **not self-approval** and not implementation handoff: no product diff/PR yet; independent QA receipt/verdict and root phase decision remain external gates. Security review, HTTP boundary, browser desktop/mobile, live workers/controller/provider/launchd, release publication, recovery witness and Tauri retirement are **NOT_RUN**. No credentials, CLI auth/trust files or transcripts were read. Worktree remains for the open task and retained evidence; no merge/setup/session adoption occurred.
