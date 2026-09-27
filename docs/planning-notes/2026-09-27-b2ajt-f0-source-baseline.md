# b2ajt F0 — source, baseline and minimum F1 seams

Owner: aperture-web-backend. Dispatch: aperture-wisp-suhh3r.
This is preflight evidence, not F1 implementation or permission to run a controller.

## Frozen source

- Worktree/branch: `aperture-b2ajt-web-local`.
- Source tested: `abeb9aedadfa10cfc6d2c42bdaeb0d74ee112c24`.
- Plan read using `git show 3933a6a:docs/planning-notes/2026-09-27-aperture-web-local-decision.md`.
- `git merge-base --is-ancestor d6d01cc abeb9ae` and the same for `5394ae0`: both exit 0. Full predecessors: `d6d01ccff401098ca1485d73f447d5130659ae83`, `5394ae035a65174237fcd0d6021c9a7f84e3e77e`.
- Content checked below, not inferred from ancestry alone. No missing freeze content identified; no imported branches/backlog. Historical PASS counts in the plan are not this run's evidence.
- Actual stack: Rust edition 2021, Tauri 2; vanilla DOM TypeScript, Vite 8; Node MCP. No React or HTTP server dependency currently. UI polls every 3s. Existing publication is desktop bundle/helpers, not an authorized web deployment. No merge/publication target inferred; this task starts at the explicit cumulative SHA, not master.

## Requirement → existing symbol/test or concrete gap

| Requirement | Evidence in tested head | Remaining phase/gap |
| --- | --- | --- |
| Durable bounded gate diagnostics, early root exit | `team_claude_launch.rs::LaunchDiagnostics`; `team_replacement_native.rs` diagnostic health/finish path; `diagnostic_native_failures_preserve_exact_phase_without_private_text`, `diagnostic_post_exec_exit_is_gone_not_timeout_or_invented_cli_exit_code`, `claude_diagnostic_poll_early_failure_timeout_and_unknown_stay_distinct` | Reuse. Does not establish a real CLI failure's cause. |
| Recovery preserves historical Unknown, bounded g1/g2 selectors | `bootstrap_authorized`, `authorize_recovery`; tests `recovery_proof_accepts_only_the_factual_quarantined_claude_bootstrap_at_g1_or_g2`, `recovery_never_widens_past_g2_and_the_smoke_reconciler_stays_g0_only` | Reuse; never automatic retry. |
| Retirement and coordination identity | `team_archive_retirement.rs::reconcile_stopped`, `inspect`; tests `retirement_reconcile_g3_preserves_unknown_and_archives_without_new_attempt`, `coordination_socket_root_attribution_never_expands_target_and_keeps_unknown_match`, `coordination_root_overlap_recycle_topology_or_identity_drift_never_exempts` | Reuse; no permission to kill unfamiliar processes. |
| Operator is not GLaDOS | `teams.rs` headless authenticated control; tests `headless_control_derives_glados_and_rejects_forged_actor_fields`, `launcher_is_not_operator_authorization` | **Plan mismatch:** operator bootstrap g0 is currently admitted. See decision below. HTTP auth/session boundary absent. |
| READY/BEADS real and complete lifecycle | `docs/runtime/managed-mcp-readiness.md`, `tests/real-bd-team-seat.test.mjs`, existing boot harness | DB fixture passes here; real two-seat cycle/server restart remains NOT_RUN, F3 with separate GO. |
| One controller, daemon adoption, no duplicate re-kick | Current `lib.rs::run` owns lifecycle; `ws_hub.rs` still kills residual listener by port; `codex_appserver.rs::spawn_app_server` returns on socket connect; watchdog re-kicks in memory | Lock seam F1; identity/adoption/intent-outcome F2. Existing code is not proof of survival across restart. No daemon started in F0. |
| Workers bound to immutable release; independent UI updates | `config.rs::default_state` uses `CARGO_MANIFEST_DIR`; launch/helper validators retain fixed paths | F2 release schema/path delta, v1 preservation, separate runtime/UI pointers. No release publication or installation here. |
| Inspect differs from Open; per-team tmux C1–C5 | `team_terminal.rs::team_open_seat` requires active exact binding; `team-terminal.test.mjs` rejects quarantined targets; `main.ts` and config use session `aperture` | F4 inspect verb/historical pane binding/per-client read-only and per-team session absent. Do not weaken Open. |
| Browser desktop/mobile, refresh session, adapter | Four service modules directly import Tauri invoke; three have injectable factories; `team-contract.test.mjs` validates strict DTOs and no auto-retry | HTTP adapter/auth and real boundary tests absent; native Node UI suites currently runner-blocked. Browser E2E NOT_RUN. |
| Remove Tauri only after full evidence | Tauri builder, bundle configuration and CLI dependencies present | F5 only after separate GO/window; no removal in F0/F1. |

## Material authority discrepancy — decision required before F1

Plan section 4.3 describes bootstrap as an existing engine denial returning `E_CONTROL_UNAUTHORIZED`. Actual call chain:

`teams::team_bootstrap_seat` → `bootstrap_seat` → `bootstrap_authorized` → `authorize_bootstrap` for g0.

`team_replacement_native.rs:147-154` accepts principal `operator` for g0. Only recovery g1/g2 uses GLaDOS-only `authorize_recovery`. `ReplacementError::AuthorizationRequired` maps to `E_REPLACEMENT_AUTHORIZATION` (`team_replacement.rs:140`), not the plan's claimed error. The test `bootstrap_is_operator_g0_only_and_never_creates_state_for_bad_selector` explicitly anchors this distinction. QA independently confirmed it (messages c6rvwm/w7c65e). No bootstrap was executed.

Root confirmed the finding and policy distinction (aperture-wisp-ybd4ju): retain the plan's *web* prohibition through an explicit HTTP denial before native dispatch, without changing the frozen engine. This is a deliberately narrower new web policy, not current-engine parity. Cipher must still freeze its exact error/status in security acceptance; test a native-call counter remains zero and no owner/attempt/process side effects. Never call the existing wrapper expecting it to deny.

## Adapter contract and smallest F1 file order (proposal, not approved edits)

1. After security gate and root GO: `src-tauri/Cargo.toml`, lockfile only for named server dependencies, `src-tauri/src/lib.rs`, new `bin/aperture-server.rs` and bounded server/auth/controller modules. No blanket dependency upgrade. Retain Tauri build/fallback; acquire controller lock before daemon startup or mutation. No live execution to validate this.
2. `agents.rs`: extract shared-state entry points preserving `require_legacy_lifecycle` (start/stop/restart/update model), validation and legacy error shape. `teams.rs`: shared-state engine/permit-store seams; actor passed by the authenticated boundary, never supplied in JSON. Reuse existing engine and DTOs. `tmux.rs` already has plain function signatures; its two registered verbs need no general command interpreter.
3. Preserve `team_*` engine/authority, owner and hub_auth freezes in F1. Root ybd4ju explicitly permits a mechanical wrapper-only seam in `team_terminal.rs`, as named by the plan. Proposed smallest clean path: expose the existing synchronous `open` core through a bounded crate-visible wrapper with the same error mapping, called under the transport's blocking executor; retain the Tauri wrapper and identical native binding checks. QA checks the exact mechanical diff. Reusing the existing public `team_open_seat` with its `tauri::async_runtime::spawn_blocking` is a documented transient alternative, not removal of Tauri. Do not duplicate native Open logic or start GUI to verify it.
4. `src/services/tauri-commands.ts`: add injection like existing factories. One finite HTTP command map plus shared transport selection used by `team-commands.ts`, `team-runtime.ts`, `team-terminal.ts`; compose before `main.ts` issues its first tmux/list request. No generic server invoke endpoint and no route for unregistered tmux helpers or GLaDOS control.
5. `vite.config.ts`, focused boundary/adapter tests, and native `package.json`/`justfile` test aliases only as dispatched. No new harness. Publication targets remain F2, not automatic consequences of introducing scripts.

20 registered commands verified in `lib.rs::generate_handler!`: 6 agent + 2 tmux + version + 10 team/terminal. Existing UI uses 16 service calls, not all 20. Preserve typed request/response parsers and exact selector envelopes (`{input: ...}` for team commands; camel-case Tauri args only translated at adapter boundary). Unknown/forged fields cannot choose actors, paths, grants, argv or shell. Writes never auto-retry; transport loss means unknown until readback. `team_archive` remains a checklist, never the mutating archive path. Legacy errors are strings, team errors `{code,message}`; normalize only at the adapter boundary under a specified contract, not by inventing backend success. Replacement permits remain process-local; loss across restart needs the phase-approved outcome/readback rule, not an inferred retry.

Native route/auth tests must exercise the real router with inert state: missing/stale session, bad Host/Origin, exchange reuse/expiry, refresh, forbidden control route, explicit bootstrap denial, no native effects on rejection. Existing TS parser tests should also run via the adapter. No source-string test substitutes for those boundaries.

## Baseline — unique retained outputs

All logs below are relative to this worktree under `tmp/f0-20260927T1400/`; no runtime/checkout build output was reused. Working tree stayed clean throughout the tests. Cargo used the existing cached dependencies offline and a task-owned target; Node dependencies were not installed or linked.

| Invocation | Result | Evidence SHA-256 |
| --- | --- | --- |
| In `src-tauri`: `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/../tmp/f0-20260927T1400/cargo-target" cargo test --lib` | Exit 0; **550 passed, 0 failed, 9 ignored**, 74.75s test time. No ignored/live opt-ins. | `cargo-baseline.log`: `4ebc038ae04d944ef0a2406508ac284c817cc7f11a4a71accf6b790284bca6ef` |
| `node --test tests/*.test.mjs` | Exit 126 before assertions: Volta shim has no default Node. **RUNNER_BLOCKED**, not product RED. | `node-baseline.log`: `cfc1be3fedea9f59012bad0f300a611e6ca4e31800e9bad1bdd6fc63c6c8eae4` |
| Root-authorized single retry (5b8dgg): prefix process PATH with `/Users/franciscomateus/.volta/tools/image/node/22.22.3/bin`, execute that absolute `node --test tests/*.test.mjs` | Exit 1; **2 pass, 9 module-load failures**. The real-bd fixture and its effective-DB gate pass; 9 UI suites cannot resolve `vite` because own `node_modules` is absent. **RUNNER_BLOCKED** for UI, not assertion regressions. | `node-baseline-native.log`: `4f09fb7afb2b3969b008b60e093a25c9aa46d7319290c77fb487a00b39f4b23c` |

No further retry performed. Proposed minimum dependency remedy: root authorizes an exact existing dependency-cache source, lockfile-compatible, copied (not symlinked) into the task worktree, plus one new baseline attempt. Otherwise leave UI NOT_RUN. No install/network fetch/global Volta change/toolchain update is authorized.

## Status

F0 evidence ready for root reconciliation; **not F0 accepted**, no F1 GO inferred. Outstanding owners: root/Cipher resolve bootstrap/web policy and remaining auth contract; root authorizes isolated dependency provisioning/retry if desired. QA reviews this evidence independently. Product diff, PR, merge, setup, session adoption, launchd, controller, real witness and Tauri retirement: **not performed**.
