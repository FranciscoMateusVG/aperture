# Hermes Lane 2 consolidation + memory sidecar backfill — DRAFT for GLaDOS review

Status: DRAFT. Nothing here has been written to the memory bank. Companion artifacts: `docs/memory-meta.seed.json` (sidecar seed, 200 keys) and `mcp-server/test/fixtures/memory-golden.json` (30 golden queries). Spec: `docs/superpowers/specs/2026-09-06-context-diet-design.md` §2-§4, migration step 2.

No secret values appear in this document or in the seed; only memory KEYS are listed.

## 1. Proposed consolidated entries (41 `hermes-lane-2-*` -> 3)

Each becomes a NEW `bd remember` entry after review; the originals stay untouched and get `superseded_by` via the sidecar `supersedes` lists below. Digests are 8-char prefixes; minted secrets are referenced only as "escrowed".

### `hermes-lane-2-consolidated-2026-07-31-status`

Body (2469 bytes, limit 2560):

```
HERMES LANE 2 STATUS as of 2026-08-01 (Peppy). BUILT: Slack DM bot for Incluir student questions. Chain: hermes (minimal distroless, holds NO privileged secret) -> gateway (OpenAI-compatible, sole holder of real OpenAI key, tools forced to incluir_lookup+incluir_query) -> broker (signs ingress+service HMAC; /lookup/by-name + /query) -> squid 6.14 -> Lane 1 app.programaincluir.org /api/bot/*. Fail-closed peer guard on broker+gateway. Source ~/projects/hermes-lane2 on Mac (NOT git), rsync'd to mini:projects/hermes-lane2; runs on Mini OrbStack. IMAGES: hermes-minimal:lane2fork Cipher-reviewed 79fedf06 (wheel 7fbdd319; supersedes b9522a86/36f4f658/eb9f3f0b), RUNNING 439dcc0a (+incluir_query); gateway hermes-openai-gateway:local Cipher-PASSED bb4274a6, RUNNING 08ccdea8 (+incluir_query, SSE relay); broker hermes-broker:local 6afec843; squid ubuntu/squid:6.6-24.04_edge@8a3baed4; builder debian:13-slim@020c0d20, runtime distroless/python3-debian13@3f056d8b. Wheel hermes_agent-0.19.0+lane2fork (CVE-2026-10221 deny-by-default patch, cryptography 48.0.1, pillow 12.3.0, hashed lock, pip check clean). PROVEN vs frozen digest: Trivy 0 CRIT/20 HIGH rows/10 unique (5 libexpat remediated pyexpat=2.8.2; 15 no-fix DoS-class); manifest EXACTLY {incluir_lookup} via read-only /etc/hermes lock; regressions x2 PASS; Feishu VEX F1-F5; probes 104/0 (isolation 11/0, DNS 8/0, hardening 32/0, PII 5/0, peer-guard 8/0 + zero upstream SYN, SSRF + allowed-host rebind denied); all 4 containers uid10000 (squid 13) CapEff0 NNP1 Seccomp2 under pinned moby v28.3.3 profile (01536f1d); container + OrbStack restart survival; gateway suite 27/27; strict mock E2E A/B; s6-root §6 waiver RESOLVED. LIVE: 07-31 14:42Z data path live per ship directive: secrets minted + escrowed (see drawer), Lane 1 Dokploy env activated (by-name 503->400->200 real StudentSummary, cpf/studentId stripped); full Slack loop working same day; 08-01 broad-read incluir_query live (61-table catalog, partial-name + deep full-record verified); gateway caps env-only 16384 tokens/40 msgs/131072 body/60 rpm/500000 daily; actor = SLACK_ALLOWED_USERS. BEADS: b6e9t, csv1x CLOSED; 2yzav satisfied by full loop. OPEN: 6vsxi P2 cold-power-loss self-recovery (OrbStack GUI-session gating); 4avov P2 live nonce/quota/rotation/cost drills; 59w92 P3 Trivy risk-acceptance sign-off; ywaiz P3 broad-read polish (~8 catalog calls/record, enrollment join broker_denied, 1024 output clip); probes 11/30 assertion realism.
```

Would supersede (26 keys):

- `hermes-lane-2-6a-final-evidence-2026-07`
- `hermes-lane-2-blockers-6-8-done-2026-07-30`
- `hermes-lane-2-broad-read-consumer-live-2026-07-31`
- `hermes-lane-2-broad-read-name-lookup-fix-2026-08-01`
- `hermes-lane-2-bundle-submitted-2026-07-30`
- `hermes-lane-2-deploy-todos-resolved-2026-07`
- `hermes-lane-2-derivative-proven-non-root-2026`
- `hermes-lane-2-distroless-0crit-achieved-2026-07-30`
- `hermes-lane-2-distroless-rebuild-state-2026-07-30`
- `hermes-lane-2-final-digest-2026-07-31`
- `hermes-lane-2-final-rebuild-2026-07-31`
- `hermes-lane-2-finalbuild-2026-07-31`
- `hermes-lane-2-full-loop-working-2026-07-31`
- `hermes-lane-2-gateway-caps-broad-read-2026-08-01`
- `hermes-lane-2-golive-datapath-live-2026-07-31`
- `hermes-lane-2-isolation-evidence-state-2026-07`
- `hermes-lane-2-next-actions-2026-07-30`
- `hermes-lane-2-operator-ship-directive-2026-07-31`
- `hermes-lane-2-patched-wheel-built-2026-07-31`
- `hermes-lane-2-peer-guard-proven-v7-2026-07-30`
- `hermes-lane-2-phase2-refreeze-2026-07-31`
- `hermes-lane-2-progress-2026-07-31b`
- `hermes-lane-2-punchlist-closed-2026-07-31`
- `hermes-lane-2-real-entrypoint-negative-rerun-2026`
- `hermes-lane-2-seccomp-done-2026-07-31`
- `hermes-lane-2-status-2026-07-31-consolidated`

### `hermes-lane-2-consolidated-2026-07-31-decisions`

Body (2483 bytes, limit 2560):

```
HERMES LANE 2 DECISIONS 2026-07-30..08-01. OPERATOR via GLaDOS: 07-30 FULL AUTONOMY (wisp-c6c1m5): stop escalating; reachability pass FIRST, rebuild only if reachable; Cipher = co-adjudicator; surface only when live-ready. 07-31 SHIP DIRECTIVE (wisp-wdoem5, final): stop Cipher-gate cycle, mint secrets, wire broker, connect; defer Mini reboot/live drills/risk-acceptance as BEADS follow-ups; boundary LIFTED, hardening stands. Mini reboot bar = unattended cold-power-loss self-recovery; auto-login = operator tradeoff. 08-01 GLaDOS approved gateway caps 16384/40/131072/60 (csv1x, ywaiz). CIPHER: 07-30 pre-live PARTIAL PASS: §6 non-root waiver DENIED (sleep-override proves netns not runtime) -> digest-pinned derivative execing Hermes as UID 10000 bypassing s6, or reject; 4 artifacts TOGETHER (digest+diff, SBOM/CVE, FULL /proc tree + cold-boot, 0-fail negatives vs real entrypoint). Work-package v2: squid->ingress reach = BLOCKER -> peer guard on normalized socket remoteAddress, never XFF, exact static IP, fail-closed; AppArmor absence = residual only with pinned seccomp; Trivy HIGHs per finding, rows AND unique; ONE zero-failure bundle. Pin #2 per-hop HMAC DROPPED = OPTION B (Hermes holds neither Incluir secret nor OpenAI key). 07-30 bundle FAIL/HOLD, 8 blockers: unfrozen artifact, no controlled-resolver SSRF, implicit seccomp, rescan all 4 images, down/up != cold boot, cost fail-OPEN, mixed-case on correct listener, tests not checked in. BASE IMAGE: derivative feasible, but reachability scan (246H/13C, 7 no-fix CRIT) -> REBUILD MINIMAL hermes-agent[slack]==0.19.0; slim refuted 0-CRIT (perl-base) -> DISTROLESS, Cipher concurred (wisp-1bo4js). wisp-fv4ekv: --force-reinstall REJECTED -> vendor-patched wheel from attested commit 3ef6bbd, close CVE-2026-10221 in-source, Feishu VEX route, pinned chain; manifest = exactly incluir_lookup via managed config. 07-31 HOLD-not-PASS on b9522a86 (controls solid; 4 go-live gates); GLaDOS OPTION A = re-freeze with NATIVE incluir_lookup tool in hermes (NOT MCP; gateway cannot reach broker by design); mock incluir + test keys = inside E2E boundary. Reconstruct tool_calls relaxation routed to Cipher, not self-certified: approved to exact spec + duplicate-id check -> PASSED bb4274a6. Seccomp: pin moby default; seccomp != LSM. Streaming: relay native OpenAI SSE, no fork patch. MODEL: gpt-4o-mini via named custom provider (api_mode openai, discover_models false). Trivy residual: owner Peppy, 90d, operator via GLaDOS.
```

Would supersede (22 keys):

- `hermes-lane-2-anomaly1-reconstruct-2026-07-31`
- `hermes-lane-2-blocker1-peer-guard-done-cipher-B`
- `hermes-lane-2-cipher-golivehold-2026-07-31`
- `hermes-lane-2-cipher-verdict-FAIL-HOLD-2026-07-30`
- `hermes-lane-2-derivative-image-determination-2026-07`
- `hermes-lane-2-distroless-rebuild-state-2026-07-30`
- `hermes-lane-2-finalbuild-2026-07-31`
- `hermes-lane-2-full-autonomy-reachability-first-2026-07-30`
- `hermes-lane-2-full-loop-working-2026-07-31`
- `hermes-lane-2-gateway-caps-broad-read-2026-08-01`
- `hermes-lane-2-go-live-work-package-v2`
- `hermes-lane-2-next-actions-2026-07-30`
- `hermes-lane-2-operator-ship-directive-2026-07-31`
- `hermes-lane-2-pre-live-review-bundle-spec`
- `hermes-lane-2-pre-live-work-package-cipher`
- `hermes-lane-2-punchlist-closed-2026-07-31`
- `hermes-lane-2-reachability-verdict-2026-07-30`
- `hermes-lane-2-review-bundle-addendum-cipher-2026`
- `hermes-lane-2-seccomp-done-2026-07-31`
- `hermes-lane-2-trivy-highs-corrected-2026-07-30`
- `hermes-lane-2-vendor-patched-wheel-directive-2026-07-30`
- `hermes-lane-2-work-package-v2-precision-pins`

### `hermes-lane-2-consolidated-2026-07-31-anomalies`

Body (2490 bytes, limit 2560):

```
HERMES LANE 2 ANOMALIES 2026-07-30..08-01. TRIVY: '2 HIGH' wrong twice: 4 rows = 2 advisories dup'd across uv+uvx; then scan was narrow: full scan of f34c42e2 = 246 HIGH/23 CRIT; v10 '0 HIGH' = exit-127 never-ran (trivy runs via docker, not a Mini binary). dpkg lags copied libexpat; pyexpat.EXPAT_VERSION is truth. SQUID 6.14: dns_v4_first obsolete; api.slack.com + .slack.com dstdomain FATAL; bad logformat tokens; pid under read_only -> /run tmpfs uid13; pinger cap_net_raw FATAL -> off; ubuntu/squid:6.6 tag nonexistent -> 6.6-24.04_edge. PROBES: bare container names = false passes; fixed probes exposed REAL squid->ingress reach via ext-net route (-> peer guard); netpos grep hung; denied CONNECT = exit 000 not 403 -> assert TCP_DENIED[_ABORTED] + zero SYN; deny-log flush race; '13 FAIL' counted FAIL=0 lines; shell-uid probe fails on shell-less image (use /proc); deny-log peer = Docker bridge-gateway IP. GATEWAY: cost fail-OPEN (reservation released on missing usage/catch) -> consume unless pre-dispatch error; root-owned state volume -> counter 503 -> pre-own uid10000; reconstruct.ts DROPPED assistant tool_calls (lenient mock hid it; real OpenAI rejects unpaired tool msg) -> sanitize + ordered/unique ids; non-streaming -> EmptyStreamError, hand-rolled SSE flaky -> native SSE relay; body cap 16384 < token cap so preparse-reject fired first; 4096 ceiling < catalog (4358 tokens); self-inflicted 429 -> rm counters.json. HERMES FORK: concurrent invoke_tool dispatched 6 inline builtins ungated (real hole); gate inert under EMPTY toolset -> deny-by-default; 12 kanban tools leaked past slack:[no_mcp] -> slack:[] + 57-name kill-list; HERMES_CONFIG is not a config override; HERMES_MANAGED=true needs memories dir -> set ''; provider 'openai' unknown -> custom provider; message.im event sub != OAuth scopes (Slack scope gap); `hermes chat` skips incluir toolset (-t incluir). DOCS: MANIFEST wheel/sdist hashes reversed; ea2a0e03 predates RUN pip check -> unusable; cached pip check -> rerun; HASH-MANIFEST 2 gens stale; Feishu F1-F5 + condition-4 SPECIFIED not RUN. Dispatch must live in hermes (only container on both nets). E2E loop used an HTTP stand-in for hermes. Earlier note conflated secret values with fingerprints (corrected). MODEL DSL: column as alias, nonexistent students.name -> description-only fix; partial name led with exact incluir_lookup -> description split. GOTCHAS: stalled build subagent had finished (docker inspect); Tailscale flaps -> nohup+poll.
```

Would supersede (25 keys):

- `hermes-lane-2-6a-final-evidence-2026-07`
- `hermes-lane-2-anomaly1-reconstruct-2026-07-31`
- `hermes-lane-2-blockers-6-8-done-2026-07-30`
- `hermes-lane-2-broad-read-consumer-live-2026-07-31`
- `hermes-lane-2-broad-read-name-lookup-fix-2026-08-01`
- `hermes-lane-2-cipher-golivehold-2026-07-31`
- `hermes-lane-2-deploy-todos-resolved-2026-07`
- `hermes-lane-2-distroless-0crit-achieved-2026-07-30`
- `hermes-lane-2-final-rebuild-2026-07-31`
- `hermes-lane-2-finalbuild-2026-07-31`
- `hermes-lane-2-full-loop-working-2026-07-31`
- `hermes-lane-2-gateway-caps-broad-read-2026-08-01`
- `hermes-lane-2-go-live-work-package-v2`
- `hermes-lane-2-golive-datapath-live-2026-07-31`
- `hermes-lane-2-isolation-evidence-state-2026-07`
- `hermes-lane-2-next-actions-2026-07-30`
- `hermes-lane-2-patched-wheel-built-2026-07-31`
- `hermes-lane-2-peer-guard-proven-v7-2026-07-30`
- `hermes-lane-2-phase2-refreeze-2026-07-31`
- `hermes-lane-2-progress-2026-07-31b`
- `hermes-lane-2-punchlist-closed-2026-07-31`
- `hermes-lane-2-real-entrypoint-negative-rerun-2026`
- `hermes-lane-2-trivy-REALITY-246-high-2026-07-30`
- `hermes-lane-2-trivy-highs-corrected-2026-07-30`
- `hermes-lane-2-vendor-patched-wheel-directive-2026-07-30`

Coverage: 41/41 original keys appear in at least one list.

Related non-`lane-2` keys deliberately left standalone (not consolidated, still live): `hermes-lane-1-activation-contract-2026-07-31`, `hermes-broad-read-broker-live-verified-2026-07-31`, `hermes-incluir-broad-read-schema-map-2026-08-01`, `hermes-operator-override-stop-cipher-gate-2026-07-31`, `hermes-gateway-model-bump-gpt-5.3-chat-latest-2026-07-31`, `hermes-developer-role-fix-label-align-2026-07-31`, `qyct9-*`, `openclaw-*`.

## 2. Tag vocabulary (controlled; 1-4 per entry)

| tag | meaning | count |
|---|---|---|
| `comms` | inter-agent messaging: send_message, inbox, ghost-text, Slack/WhatsApp bridges | 14 |
| `hub` | ws-hub process and its supervision | 4 |
| `codex` | Codex-model agent specifics | 1 |
| `launcher` | Tauri .app launch, launchd, PATH | 2 |
| `watchdog` | presence, liveness, stuck-agent detection, compaction | 6 |
| `skills` | skill registry mechanics (unused in this seed; reserved) | 0 |
| `beads` | BEADS task tooling and discipline (notes, close_task, mark_as_read) | 11 |
| `dolt` | bd/Dolt internals | 1 |
| `deploy` | shipping/cutover/build-vs-runtime, deploy verification | 53 |
| `dokploy` | Dokploy compose/env/traefik specifics | 15 |
| `infra` | hosts, containers, backups, cron, MinIO, SSH, Tailscale | 42 |
| `security` | Cipher rulings, auth models, exposure incidents, hardening | 56 |
| `credentials` | credential handling, secret stores, rotation (values never present) | 20 |
| `testing` | unit/integration tests, probes, CI, fixtures | 23 |
| `playwright` | Playwright specifics | 3 |
| `e2e` | end-to-end / round-trip proofs | 4 |
| `incluir` | Programa Incluir monorepo/product | 43 |
| `eunenem` | EuNenem / engine repo/product | 28 |
| `stripe` | Stripe | 4 |
| `inter` | Banco Inter rails | 8 |
| `process` | orchestration/operator process, handoffs, directives | 33 |
| `discipline` | banked lessons and verify-against-reality principles | 39 |
| `recon` | diagnosis/investigation technique | 9 |
| `hermes` | Hermes/OpenClaw assistant stack (lanes 1-3, gateway, MCPs) | 63 |
| `sentry` | Sentry (unused in this seed; reserved) | 0 |
| `observability` | telemetry, alerts, monitoring gaps | 9 |
| `git` | PR lifecycle, branches, merges | 18 |
| `review` | code/security review outcomes and discipline | 36 |
| `junk` | entry carries no information (hidden via sidecar) | 1 |
| `secret` | EXCLUDED from index/recall entirely (see §3 of the spec) | 10 |

Project counts: aperture 88, incluir 60, eunenem 31, general 19, beads-galaxy 2.

## 3. Standing entries (`standing: true`) — currently-binding operator/Cipher rules

13 entries. Each is inlined as a 300-byte excerpt in boot + precompact and boosted x1.5 in recall.

- `aperture-operator-standing-decision-2026-07-19-aperture` — operator order: aperture-b6ofj (default BETTER_AUTH_SECRET) is risk-accepted; never re-file/re-surface
- `credential-drawer-plaintext-read-ban-2026-08-28` — Cipher BINDING interim rule: no agent self-service reads of plaintext credential drawers
- `agent-liveness-discipline-operator-directed-2026-05-23` — operator-directed orchestrator rule: deep-peek claimed-but-unmoved agents (tail -40), stuck/working/waiting triage
- `glados-compact-at-60-proactive` — operator directive: compact any specialist at 60% context, unilaterally via tmux
- `cipher-security-rulings-on-banco-inter-credentials-2026` — Cipher formal rulings, binding until re-reviewed: Inter COBRANCA creds prod-only, never in staging
- `dokploy-p0-crossorg-write-bypass-DEFERRED-2026-08-28` — operator accepted risk; DO NOT re-escalate or re-ping the Dokploy cross-org P0
- `feature-live-doorbell-requires-evidence-not-promise` — rule for every "feature live" message to operator: attach verify command + output
- `glados-loop-idle-not-terminal-2026-07-31` — operator escalation: never stop a watch loop while a bead is in_progress with an open HOLD; run S1/S7 before stopping
- `glados-do-dont-ask-when-playbook-banked-2026-06-02` — operator-caught: when a playbook is banked, DO it, do not ask (OrbStack wedge precedent)
- `hermes-bot-ro-widened-all-tables-2026-09-06` — current operator decision (2026-09-06) overriding Cipher 61-table ruling: hermes_bot_ro reads all tables
- `correction-2026-08-23-to-eunenem-pix-rail` — operator-history rule: Stripe PIX deliberately abandoned; never recommend a Stripe support request for pix_payments
- `openai-shared-key-expo-public-exposure-2026-08-28` — Cipher adjudication still open: shared prod OpenAI key exposed, coordinated rotation mandatory (awaiting operator)
- `tick-output-requires-agent-liveness-deep-peek-not-just-pr-state` — operator-flagged tick rule: "nothing to report" requires agent-liveness peek, not just PR state

Considered and NOT marked standing: `hermes-operator-override-stop-cipher-gate-2026-07-31` and `hermes-lane-2-full-autonomy-*` (scoped to a finished deployment step), `qyct9-parked-*` (its no-deploy gate was later overridden by the ship directive), `vkyyi-796-accepted-residual-*` (task-scoped decision), `inter-cobranca-credential-handoff-spec-*` (the credential arrived the same day; also secret-tagged).

## 4. Secret-tagged keys (excluded from index and recall entirely)

No entry body was found to contain a literal credential value (regex sweep over Stripe/Slack/GitHub/AWS key prefixes, PEM private-key headers, and assignment-style credential patterns). The following are tagged `secret` because they are primarily credential-location / credential-handoff records (drawer references, secret-store maps, escrow notes):

- `two-sibling-test-walker-drawers-confused-twice-on`
- `hermes-hmac-pre-mint-deferred-2026-07-30`
- `hermes-lane-2-golive-datapath-live-2026-07-31`
- `hermes-slack-tokens-found-scope-gap-2026-07-31`
- `inter-cobranca-credential-handoff-spec-the-layer-6a`
- `inter-cobranca-credential-issued-escrowed-2026-08-23`
- `inter-eunenem-prod-creds-aperture-40twz-ingested-2026`
- `infisical-general-project-frequently-used-secrets`
- `eunen-m-2-0-stripe-setup-state-2026`
- `hermes-broad-read-db-role-contract-2026-07-31`

Borderline, left UNtagged (lesson content outweighs the credential reference; the regex `redact()` path still covers any span): `credential-drawer-plaintext-read-ban-2026-08-28` (a standing rule — tagging it secret would hide the rule itself), `credential-cleanup-ordering-peppy-error-2026-08-11`, `credential-hygiene-must-cover-the-whole-task-blast` (lists grep patterns, not values), `openai-shared-key-expo-public-exposure-2026-08-28` (fingerprint only), `correction-2026-08-23-to-eunenem-pix-rail`, `hermes-broad-read-broker-live-verified-2026-07-31`, `openclaw-incluir-mcp-live-2026-09-06`, `stripe-pix-on-eunen-m-2-0-sandbox` (Stripe account id is not a secret).

## 5. Supersedes pairs in the seed (current -> retracted)

- `process-slip-peppy-2026-08-10` -> `process-slip-peppy-2026-08-10-happened-twice`
- `pr-head-stuck-stale-merge-loses-commits` -> `pr-head-stuck-not-push-failed`
- `aperture-gui-launch-path-starvation-aperture-3x136-fixed` -> `aperture-gui-path-starvation-root-cause-aperture-3x136`
- `hermes-lane-2-status-2026-07-31-consolidated` -> `hermes-lane-2-progress-2026-07-31b`
- `hermes-lane-2-status-2026-07-31-consolidated` -> `hermes-lane-2-final-rebuild-2026-07-31`
- `hermes-lane-2-trivy-REALITY-246-high-2026-07-30` -> `hermes-lane-2-trivy-highs-corrected-2026-07-30`
- `hermes-lane-2-cipher-verdict-FAIL-HOLD-2026-07-30` -> `hermes-lane-2-bundle-submitted-2026-07-30`
- `hermes-lane-2-go-live-work-package-v2` -> `hermes-lane-2-pre-live-work-package-cipher`
- `hermes-lane-2-punchlist-closed-2026-07-31` -> `hermes-lane-2-cipher-golivehold-2026-07-31`
- `eunenem-staging-url-changed-2026-06-24-eunenem` -> `eunenem-staging-prod-dual-homed-2026-07-08`
- `eunenem-engine-worktree-install-recipe-corrected-supersedes` -> `eunenem-engine-repo-is-not-a-pnpm-workspace`
- `eunenem-env-var-reaches-process-env-via-envfile-not-compose-block` -> `eunenem-server-env-var-read-path-correct-runtime`
- `incluir-prod-backup-cadence-minio-is-weekly-sundays` -> `incluir-prod-backup-built-2026-07-17-aperture`
- `correction-2026-08-23-to-eunenem-pix-rail` -> `eunenem-pix-rail-disambiguation-prod-stripe-pix-blocked`
- `waha-mcp-all-phases-shipped-2026-08-08` -> `waha-mcp-phase-a-ack-unread-shipped-2026-08-08`
- `qyct9-parked-2026-07-30-per-operator-budget` -> `qyct9-broad-read-spec-v3-fail-converging-2026`
- `hermes-bot-ro-widened-all-tables-2026-09-06` -> `hermes-broad-read-db-role-contract-2026-07-31`

Considered and NOT paired: the `verify-against-reality-*` family (`cipher-verify-reality`, `-applied-to-auth-layers-not`, `-on-your-own-close-claims`, `verify-against-origin-main-not-local`) are four distinct sub-lessons, not revisions of one another; `anti-phantom-nonce-gate-2026-06-25-glados` ("FINAL FORM") vs `anti-phantom-nonce-replayable-verify-artifacts` describe the same evolution from different angles and both remain accurate; `close-tag-footgun-*` vs `tool-argument-close-tag-footgun-*` are two manifestations; `hermes-slack-tokens-found-scope-gap-2026-07-31` only corrects one clause of `hermes-lane-2-golive-datapath-live-2026-07-31` (handled by the anomalies summary); `retraction-2026-08-23-peppy-banco-inter-oauth` retracts a same-day claim that was never banked as its own memory; `openclaw-cutover-from-hermes-2026-08-08` removes the hermes-agent runtime but does not name the lane-2 gateway entries, so `hermes-gateway-model-bump-*` / `hermes-developer-role-fix-*` were left live (GLaDOS: consider retiring them if the lane-2 gateway is confirmed gone).

## 6. Golden set summary

30 queries: identifier 10, conflict 9, lexical 6, paraphrase 5. Every `expect_key` is live (not secret-tagged, not superseded). Identifier uniqueness was verified against the full dump (unique among live entries). Paraphrase queries share no content token with the target key or its first sentence.

## 7. Judgment calls for review

- Project of `reanimated*` / `forro-*` / `next-js-standalone-*` set to `general` (Cadence/forro have no canonical `project:` label in the taxonomy).
- `hermes-*` keys default to `aperture` unless the body is about incluir prod data/roles (then `incluir`).
- `incluir-prod-backup-cadence-*` supersedes `incluir-prod-backup-built-*`: the older note implies daily MinIO and nearly caused a false incident; the 3-2-1 layout stays reachable via `include_superseded`.
- `hermes-bot-ro-widened-*` supersedes `hermes-broad-read-db-role-contract-*` (which is also secret-tagged): the 61-table ruling is no longer the grant scope.
- `updated` was taken from the key when present, else the latest valid `2026-MM-DD` in the body; 7 entries have no date at all and omit the field (age then falls to Dolt first-seen).
- `entities` are only extracted identifiers (bead ids, PR numbers, hostnames, paths, env-var NAMES); paths under `keys/`, `.env`, `.ssh/`, and anything token/secret-shaped were excluded.
- `just recall-gate` is not a justfile target on this branch; the gate was run as `node scripts/recall-gate.mjs` from the worktree root.
