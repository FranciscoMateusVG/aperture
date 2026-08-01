# Hermes Lane 3 — Broad-Read Capability: SECURITY SPEC

**v3 — detailed spec, single-mechanism (U/P/R on-box true-VM pushed-snapshot). Rewritten to resolve Cipher's v2 12-blocker FAIL at SHA-256 `06a3bfc6…`. Every load-bearing mechanic is now pinned to a concrete, implementation-provable artifact (no remaining "proposals"). Decisions A/C/D/E/F are OPERATOR-CONFIRMED (2026-07-30, blanket "just go" via GLaDOS); SEPARATE from narrow lane jxwbd; no build/deploy clearance. Operator risk-acceptances (§2, §10, §15 hypervisor residual) are PRESERVED VERBATIM from v2 — not reopened.**

**v3 change-log vs v2 (Cipher's 12 v2 blockers, blocker → resolution):**
1. Pending contracts → **all pins concrete**: Cipher's ceilings adopted verbatim (§3, §9), internal subnets/ports fixed (§1a), 120s capability + 180s nonce TTL + 24h/26h cadence pinned. Only three values remain "pending operator confirmation" (cold-boot unlock choice §8a, audit-sink identity §7, exact incluir app-table set §4a) — each with a concrete default + fixed structure.
2. E9 → **concrete terminating ingress receiver** (§ Pushed-Snapshot / Receiver, §1a E9): P's own external NIC, source-IP filter + pinned mTLS client cert, pre-slot handshake/idle/rate bounds, confined non-root receiver, dedicated encrypted landing volume. P TERMINATES (does not forward).
3. Boot interlock → **durable launchd boot-orchestrator state machine** (§1b) with pinned interlock sequence + fail-closed.
4. Secret bootstrap + R unlock → **operator-unlock-at-boot** (§8a), no durable plaintext key, no secret-store edge; single pinned crypto construction (age v1.x X25519 + detached Ed25519 manifest).
5. Durable generation floor → **P-side sealed generation journal** outside R (§ Durable generation floor).
6. Manifest reconcile → **ONE signed manifest**, ciphertext archive-digest verified before decrypt, per-table digest check removed (§ Signed manifest).
7. Atomic swap → **single-txn schema rename** + **per-generation DEK crypto-erase** (§ Atomic swap).
8. OID pinning → **schema-qualified name + schema-fingerprint**, generation-bound OID resolution (§4a).
9. Nonce/cap durability → **atomic SET-NX + reserve-then-reconcile** durable store (§3, §11).
10. Outer-LIMIT → **real bounds** (plan-cost ceiling + cursor-FETCH-50 mid-stream + statement_timeout); outer-LIMIT explicitly rejected (§3, §4c).
11. Identity pins → **tool-manifest hash, CVE scanner id, audit-sink identity, source-export SLOs** all pinned (§7, §12, § Source export).
12. Go-live evidence → snapshot A–F set retained + tightened (§ Go-live).

- **Bead:** aperture-qyct9 (broad-read). **SEPARATE** from the narrow-lane bot (aperture-jxwbd) — this document governs infra, credentials, VMs, replica and budget that it shares with NO other lane.
- **Single authoritative mechanism:** this spec describes exactly ONE build — the **on-box, true-hypervisor, three-VM U/P/R pushed-snapshot** design. There is no streaming replica, no cloud VM, no live standby, and no `pg_is_in_recovery`/replay-lag control anywhere in this document. Any such prose has been **deleted**, not marked superseded. If you find a streaming/standby/cloud-VM reference here, it is a defect.
- **Status:** DESIGN DOCUMENT ONLY. Decisions A/C/D/E/F below are **OPERATOR-CONFIRMED (2026-07-30, blanket "just go" via GLaDOS)**. No code, no provisioning, no deploy. No build/deploy clearance is granted or implied. Go-live requires a separate Cipher PASS against the § Go-live evidence set (the snapshot A–F set), at a fresh exact hash.
- **Date:** 2026-07-30.

---

## Operator-confirmed decisions (2026-07-30, blanket "just go" via GLaDOS)

| Tag | Decision | Confirmed posture | Status |
|-----|----------|--------------------|--------|
| **A** | Replica path & VM host (§1, §6, §15) | **ON-BOX true-hypervisor three-VM U/P/R stack on the Mac Mini** (Apple Virtualization.framework / UTM — genuine own-kernel VMs, NOT OrbStack). The data lives on **R**, a third isolated VM that holds a **one-way pushed encrypted snapshot** of incluir prod — NOT a paid cloud VM, NOT a live/streaming replica, NOT co-resident with the prod primary. Source pushes; R never reaches back. **Operator explicitly accepts the shared-host hypervisor-escape residual** (an escape lands beside prod-adjacent incluir-postgres/watchtower on the Mini); his framing — *"security risks are life, minimize them as much as possible"* — means residual-accepted does NOT mean skimp. Full mechanism: §1 + §15. | **OPERATOR-CONFIRMED** (2026-07-30, blanket "just go" via GLaDOS) |
| **C** | Bulk caps posture (§3) | **CAPPED**, not unlimited — per-query AND cumulative per-event / per-UTC-day ceilings on statements, rows, bytes; bulk-collapse aggregates and unbounded pagination banned. | **OPERATOR-CONFIRMED** (2026-07-30, blanket "just go" via GLaDOS) |
| **D** | Retention & processor posture (§10) | Pursue OpenAI Zero-Data-Retention / no-model-training (ZDR/MAM, enterprise or API-agreement) for the dedicated OpenAI project. **If unattainable:** default OpenAI abuse-monitoring retention (~30 days) AND Slack retention are EXPLICITLY risk-accepted by the operator. | **OPERATOR-CONFIRMED** (2026-07-30, blanket "just go" via GLaDOS) |
| **E** | SQL-direct vs NL→SQL (§14) | **Direct SQL primary** — read-only SQL is the primary interface, transparent and audited exactly as executed. NL→SQL only behind mandatory show-SQL + fresh operator confirmation. | **OPERATOR-CONFIRMED** (2026-07-30, blanket "just go" via GLaDOS) |
| **F** | OpenAI ceiling & key (§8, §9) | **Dedicated, PROJECT-SCOPED** OpenAI key (NOT the shared General key); **50K tokens/UTC-day** cost ceiling to start. | **OPERATOR-CONFIRMED** (2026-07-30, blanket "just go" via GLaDOS) |

> **Note on A:** the confirmed mechanism is on-box (no cloud VM) and snapshot-based (no streaming replica). This spec is written *only* for that mechanism. Cipher's true-Mini-VM U/P/R conditions (bead aperture-qyct9, 2026-07-30) and her replica-host ruling (co-hosting on xerox rejected; one-way pushed snapshot into an isolated third VM required) are the architecture this spec implements.

---

## § Shape (LOCKED — operator informed decisions)

Recorded as locked, LGPD/account-security-significant posture, not infra defaults. The operator chose each explicitly and was informed of the consequence.

- **Interface:** chat + exactly **ONE** generic **read-only SQL** tool over the **ENTIRE** `incluir` database. Not typed lookups; a generic query surface.
- **Field scope:** **ALL fields returnable, UNREDACTED**, crossing to **OpenAI (processing)** and **Slack (retention)**. No field-level filtering.
- **Credentials INCLUDED:** see §2 — the ONE condition changed from Cipher's default.
- **Access:** **operator-DM-only**, fail-closed Slack allowlist = member id **`U050U6BNCS0`**, `GATEWAY_ALLOW_ALL_USERS=false`.

Any change to this shape — a second tool, a generic HTTP surface, any write/bulk-export path, or a widened Slack audience — **voids this spec** and requires re-review.

---

## §1 — Isolation & topology: the U/P/R three-VM stack (MANDATORY)

**Condition (Cipher, on-box true-VM ruling):** on-box is admissible only as **at least two genuine full VMs plus the isolated snapshot node** — never one VM with in-guest egress controls, because a compromised Hermes guest can disable its own firewall/proxy. The confirmed topology is three own-kernel VMs on the Mini under a real hypervisor (Virtualization.framework / UTM), each with an independent kernel:

- **U — UNTRUSTED (Hermes) VM.** Runs the Hermes chat/LLM workload and the Slack message-relay client. **Exactly ONE NIC**, on a private internal L2 vmnet shared only with P. **No** NAT/shared/bridged NIC, **no** external DNS resolver, **no** host integration, **no** default route. U's only reachable peers are P's fixed application listeners. U holds **no real OpenAI key and no DB credential** (§5, §8).
- **P — POLICY (trusted) VM.** Runs the fixed query **broker**, the **OpenAI egress gateway**, the **Slack proxy/egress**, the **snapshot ingest receiver**, the **audit client**, the **capability minter**, the **cumulative counters**, and holds **all real credentials and the snapshot decrypt key** (in tmpfs, §8). P sits on the U↔P private segment AND on the R-facing private segment AND has **one external attachment** for allowlisted egress + inbound snapshot ingest. P exposes **no generic router/NAT/forwarder** to U; generic IP forwarding is **disabled**; only P's application-level listeners originate allowed outbound connections.
- **R — REPLICA/SNAPSHOT-NODE VM.** Holds the pushed snapshot **at rest** and serves read-only SQL to the broker. **Exactly ONE NIC**, on a private internal L2 vmnet shared only with P. R **initiates no outbound connection to anything** — not xerox/prod, not the tailnet, not the Mini host, not the internet. R receives DB connections from P only (§2). R's disk/WAL/temp/staging are encrypted (§3, §8).

**PF anchor = OUTER backstop + boot interlock only.** A persisted, version-controlled macOS PF anchor is an *outer* enforcement layer and a boot interlock — **not** the sole boundary. Primary enforcement is the separate-VM / private-L2 topology above (per Cipher: PF alone is rejected — shared/NAT vmnet is host-connected by design, dynamic Slack/OpenAI FQDNs don't map to static IP rules, and rule behavior drifts across macOS updates).

**Fail-closed boot order (no auto-fallback).** Enforced by the durable boot-orchestrator state machine in **§1b** (a launchd `LaunchDaemon` on the Mini host). U and R **autostart is disabled**; nothing but the orchestrator may start a workload VM. If any precondition (PF active, vmnet identity, R unlock+health, P policy/listener/key/counter attestation) is unmet, **U stays stopped** and no query path exists. **There is no fallback to shared/NAT networking, ever.**

**Host/VM hardening (all three VMs):** pinned UTM/Virtualization.framework build; minimal pinned+patched LTS arm64 images; own-kernel proof per VM; FileVault + guest disk encryption; Secure Boot / verified boot where supported; **no** shared folders / VirtioFS, clipboard/SPICE agent, host SSH agent, USB/audio/camera, Docker socket, host port-forwarding, or host service mounts. Admin only via local console or a separately-authenticated management plane **U cannot reach**.

### §1a — Authoritative U/P/R directed-edge graph

Every allowed edge is **one-directional** (initiator → listener). Anything not listed is **denied and proven denied** (§6, § Go-live evidence). **Addresses/ports are PINNED** (Cipher accepted the security-neutral internal pins `10.55.0.0/24` U↔P, `10.55.1.0/24` P↔R, ports `8443/8444/8445/5432`, subject to collision/config proof in go-live evidence A). The E9 inbound external address/port is pinned below; only the source-endpoint *identity* is by cert (not IP), plus a source-IP allowlist (§ Pushed-Snapshot / Receiver).

| # | Initiator | → | Listener (node:addr:port) | Purpose | Notes |
|---|-----------|---|---------------------------|---------|-------|
| E1 | **U** | → | **P** broker RPC `10.55.0.2:8443` (mTLS) | submit `{capability, exact-SQL-bytes}` | single op; no passthrough/admin |
| E2 | **U** | → | **P** OpenAI gateway `10.55.0.2:8444` | LLM calls | gateway reconstructs upstream body (§9) |
| E3 | **U** | → | **P** Slack relay `10.55.0.2:8445` | send replies / long-poll inbound DMs | U-initiated only; P never dials U |
| E4 | **P** OpenAI gateway | → | `api.openai.com:443` (external) | upstream LLM | fixed SNI/authority allowlist |
| E5 | **P** Slack proxy | → | Slack Socket Mode host family `:443` (external) | wss events | P holds Slack tokens |
| E6 | **P** audit client | → | audit sink FQDN `:443` (external, mTLS) | append-only audit (§7) | append-only creds; no other egress on this route |
| E7 | **P** broker (query role) | → | **R** Postgres `10.55.1.10:5432` (mTLS/SCRAM) | read-only SQL | R live snapshot DB |
| E8 | **P** importer (import role) | → | **R** Postgres `10.55.1.10:5432` (mTLS/SCRAM) | staging restore + swap | separate role from query role |
| E9 | **SRC** (xerox exporter) | → | **P** ingest receiver `P-ext:9443` (mTLS, client-cert-pinned) | source-initiated snapshot push (P **terminates**, does not forward) | The ONLY inbound edge to P; source-IP allowlisted to xerox's pinned egress addr; §E9-detail |

**Explicitly denied directed edges (proven absent):** U→R (no DB route); U→any external (no external NIC/DNS/default route); R→anything (R initiates nothing — R→SRC/xerox, R→tailnet, R→Mini-host, R→metadata, R→internet all denied); P→SRC/xerox/prod (P receives the push, never dials prod); P→Mini-host / LAN / RFC1918 / CGNAT-tailnet(100.64/10) / link-local(169.254/16) / metadata / ULA(fc00::/7) / link-local-v6(fe80::/10) / multicast / raw-IP / UDP / arbitrary-public over IPv4+IPv6 (only E4/E5/E6 external destinations + E9 inbound are permitted); any inbound to U (U accepts return traffic only on U-initiated connections).

### §1b — Durable boot-interlock supervisor (concrete state machine)

**Mechanism (named + concrete).** A macOS **`launchd` `LaunchDaemon`** — `com.aperture.hermes-boot-orchestrator` — installed at `/Library/LaunchDaemons/com.aperture.hermes-boot-orchestrator.plist`, `RunAtLoad=true`, `KeepAlive` on non-zero exit, running as a dedicated non-login system account. It is the **only** starter of the workload VMs. The UTM/`vmnet` autostart of U and R is **disabled** (no `AutoStart`, no login-item, no `vmctl` autoboot); only the orchestrator issues the start command per VM. This survives cold boot because `launchd` re-runs it on every boot and it **re-asserts the full interlock every time** (not a one-shot).

**Interlock state machine (fail-closed at every gate — a failed gate leaves U stopped and exits non-zero; there is no fallback path):**

1. **G0 — PF backstop + fabric identity.** Load the version-controlled PF anchor (`/etc/pf.anchors/aperture-hermes`, anchor name `aperture.hermes`); **verify the active ruleset hash** equals the pinned hash via `pfctl -a aperture.hermes -sr` (atomic load-then-verify; a partial/failed load aborts). Verify the two `vmnet` segments exist with the pinned identities (`10.55.0.0/24` U↔P host-less internal, `10.55.1.0/24` P↔R host-less internal) and that no unexpected NIC/bridge is attached. Any mismatch → **STOP, U never starts.**
2. **G1 — R unlock + health.** Unlock R's encrypted volume with the operator-supplied secret from §8a (cold boot) — no auto-unlock. Boot R. Wait for R's read-only health probe (Postgres up, `live` schema present, schema-fingerprint matches the pinned manifest, §4a). Timeout/failure → **STOP.**
3. **G2 — P key-seal + policy attestation.** Boot P. P derives its sealed key bundle into **tmpfs only** from the operator secret (§8a): snapshot decrypt (age X25519) private key, audit HMAC key, capability-signing key, R-DB SCRAM creds, source-push mTLS trust anchor, OpenAI/Slack tokens. P then runs self-tests and **attests**: guest firewall rules loaded, the exact listener set bound (broker 8443, OpenAI gateway 8444, Slack relay 8445, ingest receiver `P-ext:9443`), allowlists loaded, **decrypt-key + capability-key + audit-key present in tmpfs**, durable counter/nonce store reachable (§11), and the **durable generation floor** readable + signature-valid (§ Durable generation floor). Broker establishes its R connection and passes the canary probe. Any attestation failure → **STOP.**
4. **G3 — U start.** Only after G0–G2 all pass does the orchestrator start **U**. U comes up with no external NIC, no external DNS, no default route; its only reachable peers are P's listeners.

**Re-assertion + renumber.** On any of: full Mac cold boot, hypervisor restart, P restart, R restart, Tailscale restart/renumber, or interface renumber, `launchd` re-runs the orchestrator, which re-executes G0–G3 from the top. If a running dependency dies (P or R exits), the orchestrator **stops U** and re-enters the interlock rather than leaving U attached to a degraded fabric. This is proven in go-live evidence A (each disable-one-precondition drill leaves U stopped, never NAT-fallback).

---

## § Pushed-Snapshot mechanism (source → P → R), fully pinned

**Status: OPERATOR-CONFIRMED (decision A).** This is the authoritative, complete mechanism for how incluir data reaches R. Nothing here is "flagged open" — every sub-item is pinned to a concrete mechanism. Values marked *proposed* are the pin-candidates; the mechanism is fixed.

### One-way authenticated push (transport)

- **Direction & initiator.** The **source (xerox) initiates** the connection outbound to **P's ingest receiver** (edge E9, `P-ext:9443`, mTLS). R never receives from the source and never dials out; P is the only node that touches both the source-facing ingest and R.

**§E9-detail — the terminating ingress receiver (concrete).**

- **P TERMINATES the push — it does NOT forward it.** This resolves the apparent conflict with the "no forwarding / P is not a router" statement (§1, §6): P is not a router/NAT/forwarder for U, and generic IP forwarding stays disabled. E9 is an **application-terminated ingress**: the receiver on P accepts the connection, authenticates it, reads the encrypted archive to a landing file, verifies the signed manifest + ciphertext digest, **decrypts in memory**, and **imports into R itself** (E8). No byte is passed through to another host; the push ends at P. E9 (inbound) and E8 (P→R DB) are distinct edges — there is no packet-level forwarding between them.
- **P's external attachment (pinned).** P has ONE external attachment — a dedicated bridged `vmnet` NIC, address `P-ext` (**the exact routable address is pending operator confirmation — it depends on where the Mini sits on the network; the port `9443`, the bind, and the filters below are pinned**). The ingest listener binds **only** `P-ext:9443`. This is P's own NIC receiving on its own port — it is **not macOS host port-forwarding** (no `pfctl rdr`, no host→guest forward), so it does not violate the §1 "no host port-forwarding" hardening rule. E9 is the **ONLY inbound edge to P** (E4/E5/E6 are outbound; nothing else listens).
- **Confined, least-privilege receiver process.** The receiver runs as a **minimal, memory-safe, non-root, capability-dropped** service under a **seccomp/AppArmor** profile that permits only: bind `9443`, accept, read socket, write to the dedicated landing volume, and signal the importer. It has **zero access to the broker sockets and zero access to P's tmpfs secrets** (separate uid/namespace; the decrypt happens in a distinct importer step that reads the landing file — the network-facing process never holds the decrypt key). It writes only to a **dedicated encrypted landing volume** (`/srv/ingest-landing`, its own DEK), never to `/`, never to broker or secret paths.
- **Single fixed operation.** The receiver accepts exactly one operation: "receive one encrypted snapshot archive + its detached signed manifest." **No SSH shell, no command channel, no file-browsing, no `rsync --server`, no arbitrary path write.**
- **Source authentication — two independent factors + IP allowlist.** (1) **mTLS** with a dedicated source client certificate (pinned issuer + pinned subject); any other client cert is rejected at handshake. (2) **Source-IP allowlist** — the connection must originate from xerox's pinned egress address (in addition to, not instead of, the cert). (3) The manifest is separately **signed by the source Ed25519 signing key** (payload auth, independent of transport auth).
- **Anti-slow-hold bounds enforced BEFORE the concurrency slot is taken** (so an internet client that passes neither cert nor IP filter, or a slow-loris, cannot occupy the single ingest slot): **TLS-handshake timeout = 10s; header/manifest read timeout = 15s; read-idle timeout = 20s; minimum sustained throughput floor** (a transfer slower than the floor is dropped); **connection rate-limit** per source addr. Only after mTLS + IP + manifest-signature pre-checks pass does the receiver **claim the single import slot**.
- **Payload bounds (PINNED — Cipher's ceiling adopted).** **Max archive size 512 MB**, **max wall-clock 15 min**, **concurrency 1** (a second push while one is in flight is rejected, not queued). A push exceeding any bound is aborted and the landing file discarded; the last-good live snapshot is untouched. (Raise only from measured dump-size evidence, operator-approved.)
- **Stateful, return-only.** The receiver returns only a terminal status to the source (accepted/rejected + reason-class); it exposes no state the source can drive beyond delivering the one archive.

### Signed manifest + single archive-encryption construction (ONE construction, no ambiguity)

- **ONE crypto construction (pinned — not "age or libsodium").** Archive encryption is **`age` v1.x (the reference `FiloSottile/age` implementation, pinned version+digest), X25519 recipient** (P's recipient public key). Manifest signing is a **detached Ed25519 signature** over the canonical manifest bytes. These are the only two constructions; the earlier "age / libsodium sealed box" ambiguity is removed.
- **Who does what:** the **source** creates the archive (`pg_dump --format=custom **--no-owner --no-privileges**`), **`age`-encrypts** it to P's X25519 recipient public key, computes **`SHA-256` over the resulting CIPHERTEXT archive** (the immutable bytes that will travel the wire), builds the **manifest** embedding that ciphertext digest, and **Ed25519-signs the manifest**. The **source holds only P's recipient *public* key + its own signing private key** — it cannot decrypt anything. **P holds the age recipient *private* decrypt key in tmpfs only** (§8, §8a). **U receives no key; R retains no decrypt key.**
- **ONE consistent manifest (reconciled — blocker 6).** The signed manifest binds **exactly**: `{ source-db-identity = incluir_hono, as_of, monotonic generation, schema-fingerprint, archive-content-digest }`, where:
  - **`schema-fingerprint`** = `SHA-256` over the canonically-ordered set `{schema.table.column:pg_type}` (relations sorted by schema-qualified name, columns by ordinal) — the same fingerprint pinned in §4a. It replaces the loose "ordered migration list + schema hash"; the ordered migration list is still carried as descriptive metadata but the **fingerprint is the enforced value**.
  - **`archive-content-digest`** = `SHA-256` of the **ciphertext** archive (pinned: the digest covers ciphertext, not plaintext).
  - **No per-table content digests.** The v2 conflict (loose manifest at the old line 89 vs a per-table content-checksum-parity check at the old line 102) is resolved by **removing the per-table content-checksum check entirely**. Integrity rests on: authenticated ciphertext digest + `age` AEAD (tamper on ciphertext fails decrypt) + `pg_restore` success under `--single-transaction --exit-on-error` + schema-fingerprint match + relation/constraint/index/sequence presence + per-table **row-count** validation (row counts ARE in the manifest; content checksums are not).
- **TOCTOU-free verify-then-decrypt.** P streams the ciphertext to the immutable landing file, computes its `SHA-256`, checks it equals the signed `archive-content-digest`, **then decrypts-and-imports FROM THAT SAME landing file descriptor** — so the bytes verified are exactly the bytes decrypted (no re-read/swap window).
- **`as_of` semantics (blocker 5).** `as_of` = the **source MVCC snapshot acquisition time**: the exporter opens a transaction, calls `pg_export_snapshot()`, records the wall-clock at acquisition as `as_of`, and runs `pg_dump --snapshot=SNAPID` against that exact snapshot (wrapper-controlled). **Clock-skew bound = ±120s**: P rejects the push if `as_of` is more than 120s in P's future (skew/forgery) — freshness math (§ Freshness) uses `as_of`.
- **Replay / rollback / forgery rejection (fail-closed).** P rejects an archive if: the manifest Ed25519 signature fails; the landing-file ciphertext `SHA-256` ≠ the signed `archive-content-digest`; the `generation` is **≤ P's durable generation floor** (non-monotonic / replay / rollback — floor is P-side, see below, NOT R's writable metadata); `as_of` is more than 120s future-dated; or the schema-fingerprint is not the pinned/accepted value (drift). A rejected push does not swap in.

### Durable generation floor (P-side, sealed, outside R) — blocker 5

- **Where.** P persists, in **P's OWN durable encrypted store** (`/srv/p-state/generation.journal`, sealed under the operator-unlock key material, §8a) — **NOT on R** — an append-only journal whose head is the **last-ACCEPTED** `{ generation, as_of, schema-fingerprint, archive-content-digest, full signed source-manifest bytes }`.
- **Why.** A destroyed/rebuilt/rolled-back R cannot reset the generation to zero and replay an old snapshot: acceptance requires **strictly-greater-than-floor** generation, and the floor lives on P, sealed, surviving any R rebuild. **A writable/compromised R metadata row is NOT authentication.**
- **Per-query check.** Before every query the broker (a) reads R's metadata row (`as_of`/generation/schema-fingerprint written during import) **and** (b) re-verifies it against P's durable floor + re-verifies the stored source Ed25519 signature. If R's metadata disagrees with P's sealed floor (rollback, tamper) → **fail closed**.
- **Rebuild-recovery.** After an R rebuild, the floor persists on P; the next push must exceed it. The floor advances only on a fully-verified accepted swap (below), written **after** commit.

### P-authorized streaming decrypt → import into R (no plaintext at rest)

- P **streams-decrypt** the (digest-verified) landing file — `age` decrypt in memory on P, piped directly into `pg_restore` — so **no decrypted plaintext archive is ever written to disk** on P or R. R's base DB/WAL/temp volumes are **encrypted at rest** (§8); each snapshot generation's table data additionally lands on its **own per-generation encrypted virtual disk / DEK** (below). The only place snapshot plaintext exists is inside R's encrypted Postgres storage.
- The import connects P's **importer role** (E8) to R and restores into a **non-visible `staging` SCHEMA** (below), inside the single fixed database `incluir_snap`. The **broker query role** has `search_path` locked to the `live` schema, cannot see `staging`, and has **no write privilege** anywhere (§4b).

### Atomic swap: single-transaction schema rename + per-generation crypto-erase (blocker 7)

**One database, data in a schema.** The single database `incluir_snap` holds the live dataset in schema **`live`**; the broker's `search_path` is locked to `live` and it connects to the fixed database name. Imports land in schema **`staging`**. This replaces the v2 two-`ALTER DATABASE`-rename design (which was **not** atomic — two separate operations).

**Per-generation encrypted storage (for genuine selective crypto-erase — blocker 7).** Each generation's table data is created in a **dedicated tablespace `snap_gen_N` backed by its own per-generation encrypted virtual disk with its own DEK**. `pg_restore` targets the `staging` schema whose tables are created in `snap_gen_N1`. Because a schema **rename** relabels the namespace without moving heap data, the live data physically resides on generation-N's encrypted disk. Dropping a generation = **`DROP SCHEMA … CASCADE` then destroy that generation's DEK + tear down its virtual disk** → the bulk table/index heap is crypto-erased **without touching the shared R volume**. (Honest residual, stated precisely per Cipher: WAL/catalog blocks on R's shared base volume are covered by R's full-volume encryption and are destroyed on a full R-volume rebuild, not by the per-generation DEK; the per-generation DEK crypto-erases the sensitive table/index heap, which is the bulk of the dataset. This is the "state the weaker deletion guarantee precisely" path, not the infeasible "DROP DATABASE crypto-erases shared blocks" claim.)

**Journaled swap state machine (single-txn commit boundary).** A journaled service-level state machine (`/srv/p-state/swap.journal`, states `importing → verified → swap-committing → swapped → old-erased`) drives:

1. **Stage.** `pg_restore --schema=staging --exit-on-error --single-transaction --no-owner --no-privileges` into `staging` on `snap_gen_N1` (any restore error aborts the whole transaction; nothing partial survives).
2. **Verify (pre-swap).** Against the authenticated manifest: **schema-fingerprint match** (§4a), expected relations/constraints/indexes/sequences present, per-table **row-count** parity. Any mismatch → **abort, `DROP SCHEMA staging CASCADE`, destroy the gen-`N+1` DEK, keep `live` untouched, alert.**
3. **Drain + freeze.** The broker (single-concurrency, §4c) stops accepting new queries and drains the at-most-one in-flight query (bounded grace ≤ its 5s `statement_timeout`, then cancel+rollback). The maintenance/importer role holds the swap; **all broker query-role connections are terminated/revoked** for the swap window (pin the maintenance role = `snap_admin`, E8; the query role is separate, E7).
4. **Atomic swap — ONE transaction (the whole point):**
   ```
   BEGIN;
     ALTER SCHEMA live    RENAME TO old_gen_N;
     ALTER SCHEMA staging RENAME TO live;
     -- in-txn canary probe against the now-live schema:
     SELECT 1 FROM live.canary_relation LIMIT 1;
     SELECT as_of, generation, schema_fingerprint FROM live.pinned_meta ;
   COMMIT;   -- or ROLLBACK on any probe failure
   ```
   Schema renames are **transactional DDL in PostgreSQL**, so both renames + the canary probe **commit atomically or roll back atomically**. The broker's fixed `search_path=live` sees the new dataset only after COMMIT. There is **no torn state** and **no window where `live` is absent**: on ROLLBACK, `old_gen_N` reverts to `live` automatically (prior snapshot fully restored, service resumes on old).
5. **Post-commit health + resume.** After COMMIT the broker reconnects/resumes; the durable **generation floor (§ above) is advanced to N+1** (written to P's sealed journal **after** commit).
6. **Destroy old (crypto-erase).** `DROP SCHEMA old_gen_N CASCADE;` then **destroy generation-N's DEK + tear down its virtual disk**. Bounded old/staging material exists **only** during the swap window; it never accumulates.

**Crash matrix (single commit boundary — simpler + genuinely atomic):**
- Crash during **stage/verify** (state `importing`/`verified`) → `live` untouched; boot-reconcile drops orphan `staging` + destroys the orphan gen-`N+1` DEK.
- Crash **before COMMIT** (state `swap-committing`) → the uncommitted transaction is rolled back by Postgres crash recovery; `live` is intact (rename never committed). Boot-reconcile drops `staging` + destroys gen-`N+1` DEK.
- Crash **after COMMIT, before old-erase** (state `swapped`) → new `live` is correct and complete; boot-reconcile finds `old_gen_N` and completes the crypto-erase (DROP SCHEMA + destroy gen-N DEK).
- A failed **in-txn canary probe** → ROLLBACK → prior `live` restored atomically before any service resumes (Cipher's "failed candidate probe must restore old before service").
- A failed or stuck import **never** reaches COMMIT; the last successfully-verified-and-committed snapshot remains queryable.

### Cadence + hard stale-refusal (fail-closed) — see §5-freshness below

Push cadence and max-age are pinned in **§ Freshness** (24h push / 26h hard refusal). The broker checks the authenticated `as_of` + `generation` **before every query**.

### Deletion / rebuild

To reset: **destroy R's encrypted snapshot volume, then trigger a fresh source push.** Because the flow is one-way and source-initiated, wiping/rebuilding R touches nothing on the source side — prod is unaffected. This is the clean-rebuild-from-IaC path (§13).

### Source-side isolation guarantees

- **Never corrupts prod:** the export is a read-only operation against the source (§ Source export, below).
- **Never blocks prod:** the push is **fire-and-forget from prod's perspective** — prod does not wait on, retry against, or degrade service for R's ingest outcome. A stuck/failing receiver has **zero** effect on prod's live traffic (proven in evidence B/F).

---

## § Source export: least-privilege, resource-bounded (Cipher blocker 6)

Read-only + fire-and-forget is **not** proof of zero source impact. The exporter on xerox is bounded so it cannot degrade or block prod:

- **Least-privilege export role** on the source primary: a dedicated `incluir_export` role — `NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION`, **explicit `SELECT`-only** on exactly the tables in the export set, **no write / DDL / function-execution** beyond what `pg_dump` needs to read. It cannot harm prod even if the export host is compromised.
- **One concurrent job** (a lock/PID guard; a second export while one runs is refused — **no retry storm**).
- **Wrapper-controlled MVCC snapshot.** The exporter opens a transaction, `pg_export_snapshot()`, records `as_of` at acquisition, and runs `pg_dump --snapshot=SNAPID --format=custom --no-owner --no-privileges` against that exact snapshot (so `as_of` is the true MVCC acquisition time, § manifest).
- **Measurable SLOs (PINNED — blocker 12; absolute "cannot degrade" wording replaced with numbers to prove):**
  - **`statement_timeout = 300s`, `lock_timeout = 1s`** (never blocks on / holds locks against prod writers), **overall job timeout = 15 min** (matches the ingest 512 MB/15 min bound).
  - **CPU/I-O caps:** run under `nice 19` + `ionice class idle` (or cgroup `cpu.max` ≤ 1 core-equivalent and low `io.weight`); **push bandwidth cap ≤ 50 Mbps**.
  - **Load guard:** skip the run if prod 1-min load average exceeds a pinned threshold (default: **> 0.70 × core-count**) OR prod query p95 is already elevated.
  - **Allowed prod p95 latency delta during export ≤ 5%** — measured and proven in evidence F; exceeding it aborts the run.
  - **DDL/migration exclusion window:** the export must NOT run during a migration deploy; it checks a migration-lock/flag and **aborts if a migration is in progress** (`ACCESS SHARE` locks from `pg_dump` can delay DDL even while writers proceed).
  - **Off-peak schedule (default, pending confirmation):** nightly **02:00–04:00 America/Sao_Paulo**.
  - **Prefer an existing verified backup:** if a verified nightly backup artifact already exists, export from that rather than re-reading prod.
- **Abort behavior:** on timeout/error/lock contention/load-guard trip/migration-in-progress the export **aborts cleanly** (no partial push initiated) and alerts; it does not auto-retry in a tight loop.
- **Evidence (F):** (a) prove a **failed or stuck receiver cannot block prod** (fire-and-forget), and (b) **load-test** that the export job itself cannot materially degrade prod (bounded CPU/I-O/lock footprint).

---

## § Freshness / as-of semantics — FAIL CLOSED (Cipher blockers 4 & 5)

The snapshot is **stale-by-design** (not a live replica) and this spec **fails closed** on staleness. There is no `pg_is_in_recovery`, no streaming lag — freshness is measured as **snapshot age from `as_of`**.

- **Cadence (proposed initial posture):** push **at least every 24h**.
- **Warn:** immediately on any push/verify failure, and again when snapshot **age ≥ 24h**.
- **HARD stale-refusal (fail-closed):** the broker **refuses ALL queries** when snapshot **age > 26h**. It does **not** serve stale data.
- **Pre-query metadata check (every query):** before executing any SQL the broker reads `as_of` + `generation` from R's pinned metadata table **and verifies it against P's durable generation floor + the stored source Ed25519 signature** (§ Durable generation floor) — **R's writable metadata alone is NOT trusted**. If metadata is **missing, invalid, below/behind P's floor (rollback), future-dated (> ±120s skew), or stale (age > 26h)** → **query fails closed**, refused with a "stale/invalid snapshot" error-class.
- **Surfaced everywhere:** every served answer and every audit row carries **`as_of` + snapshot age + generation** (never streaming lag). The bot can report the current `as_of`/age/generation on request.

---

## §2 — Data-boundary: credentials INCLUDED (operator override of default-exclude)

**This is the ONE Cipher-condition changed from her default.** Cipher's default posture excludes credential/secret material from the replica and query surface. The operator overrode that default.

**Recorded informed decision (stated plainly and distinctly):**

> The operator was asked directly whether credential and secret material should be in scope, and **enumerated the categories himself** — session tokens, OAuth access tokens, OAuth refresh tokens, OAuth id tokens, passwords, and verification tokens. He **KNOWINGLY ACCEPTED the full account-takeover risk** this creates, **on top of** the previously-recorded acceptance of all-PII exposure. This is an explicit, informed operator decision, not an oversight and not an infra default.

**Concretely, the following ARE in the replica and in the query surface (unredacted, to OpenAI + Slack):**

- `session.token` (live session tokens → session hijack).
- `account.access_token`, `account.refresh_token`, `account.id_token` (OAuth → third-party account takeover; refresh tokens survive rotation).
- `account.password` / any password-hash column (offline cracking / credential stuffing).
- `verification.value` (email/reset verification tokens → account recovery hijack).
- Any signing-key / secret / API-key columns present in the schema.

**Account-takeover acceptance (distinct from the PII acceptance):** exposing the above to OpenAI (processing) and Slack (retention) means any retained copy is sufficient to **impersonate users, hijack live sessions, take over linked OAuth accounts, and defeat account recovery** for the ~1042-student population and any staff/admin accounts in the same DB. The operator accepted this specific, elevated risk in full.

> **Account-takeover consequence (Cipher's requirement, stated verbatim):** Because the broad-read surface includes unredacted credentials and full PII, a compromise of the replica, broker, LLM path, or Slack channel can immediately enable impersonation or credential replay against any account in the system. This is an explicit, operator-accepted residual — the exfil channel (bot answers → OpenAI + Slack) IS the intended feature and no isolation reduces it.

**Every other Cipher condition below still holds fully.** The credential inclusion changes *what data is in scope* — it does NOT relax isolation (§1, §6), caps (§3), the AST allowlist / role hardening (§4), the broker split (§5), audit (§7), secrets handling (§8), the OpenAI ceiling (§9), retention posture (§10), actor authenticity (§11), tool-absence (§12), lifecycle (§13), or NL-gating (§14). If anything, §7/§10/§13 tighten *because* credentials cross.

---

## §3 — Bulk caps [OPERATOR-CONFIRMED C = CAPPED]

**Rationale:** with credentials + full PII in scope, an unbounded export is a **single-shot dump of the entire DB** — every session token, every OAuth token, every password hash, all student PII — in one answer. Caps are the exfil-rate control; they are SEPARATE from the OpenAI token ceiling (§9), which is only cost control.

**Two ceiling layers, both enforced by the broker (§5). Values below are PINNED — Cipher's recommended security-starting ceilings, adopted verbatim (v2's 1,000-row/50k/16MB values are REJECTED as disproportionate to the small incluir DB and are replaced). Raise any value only from measured dataset-size / exfil-time evidence, operator-approved:**

1. **Per-query ceilings** (single query rejected/cancelled):
   - **≤ 1 statement** (AST-enforced, §4a).
   - **Max expanded result rows = 100.** Enforced via a **server-side cursor with FETCH chunk = 50 rows**; the moment the running total would exceed 100, the statement is **cancelled + the READ ONLY transaction rolled back** (cancel *before* materialization — do not buffer-then-truncate).
   - **Max serialized result bytes = 64 KB** (the exact serialized bytes that would cross to OpenAI/Slack). Checked on the serialized output as it streams; on breach → cancel + rollback.
   - **`statement_timeout = 5s`, `lock_timeout = 1s`, `idle_in_transaction_session_timeout = 10s`** (§4c).
   - **Plan-cost ceiling = 1,000,000** (EXPLAIN estimated total cost; **mandatory, not optional** — reject before execution above the bound).
2. **Cumulative ceilings** (rejected once the running total for the window is exhausted):
   - **Per-Slack-event:** ≤ **1** statement, ≤ **100** rows, ≤ **64 KB** (one operator message = one bounded query; it cannot fan into a query loop — reinforced by §14 no-autonomous-follow-up).
   - **Per-UTC-day:** ≤ **50** statements, ≤ **1,000** rows, ≤ **2 MB**, with **durable counters** (§11 store) that persist across VM/broker restart and reset atomically on the UTC-day boundary. **Fail closed if the counter store is unavailable.**

**Atomic reserve-then-reconcile (blocker 9 — SAME pattern as the §9 OpenAI gateway).** Before a query executes, the broker **atomically reserves the worst-case allowance** (1 statement + 100 rows + 64 KB per-query worst case) against the per-event and per-UTC-day budgets in the durable store; the query runs only if the reservation succeeds. **After** the durable terminal outcome it **reconciles downward** to the actual rows/bytes. **Crash or ambiguity consumes the full reservation** (fail-safe — never under-counts an exfil). Restart mid-query / mid-stream is tested (go-live). If the durable store is unavailable, **fail closed** (no reservation → no query).

**Cancel-and-rollback while streaming.** When any cap trips mid-result, the broker issues a statement cancel and the READ ONLY transaction (§4) is rolled back. No partial oversized payload is forwarded to the model or Slack.

**Outer-LIMIT is NOT a bound (blocker 10) — real bounds only.** An outer `LIMIT` does **not** bound an aggregate or an expensive scan: `SELECT count(*) FROM t LIMIT 100` scans all of `t` (the `LIMIT` caps the 1-row output, not the input). The row/byte caps alone therefore cannot bound an aggregate. The **real** bounds that DO hold are: (a) the **mandatory pre-execution plan-cost ceiling** (rejects the whole-table scan before it runs), (b) the **cursor-FETCH-50 running row/byte totals enforced mid-stream** with cancel+rollback on breach, and (c) `statement_timeout`. Additionally the AST layer (§4a) **only permits a scalar aggregate when its input relation itself carries a `LIMIT` within the per-query row cap** (LIMIT *inside* the aggregate's input subquery/CTE); a bare aggregate over an unbounded scan, or an outer-LIMIT wrapping an aggregate, is **DENIED**.

**BANS (AST-enforced, §4a) — aggregate boundedness is MECHANICAL, not advisory:**

- **The full bulk-collapse aggregate set is DENIED BY DEFAULT** — `json_agg`, `jsonb_agg`, `string_agg`, `array_agg`, `json_object_agg`, `jsonb_object_agg`, `xmlagg` (and any set-collapsing aggregate), because they collapse an arbitrarily large set into one row/value and defeat the row cap. Only **scalar bounded aggregates** (`count`, `min`, `max`, `sum`, `avg`) over a `LIMIT`-bounded input are allowed; no allowlisted AST shape permits an unbounded collapse.
- **Unbounded pagination banned.** Every query must carry an explicit `LIMIT` within the per-query row cap; cumulative pagination across a window is itself bounded by the per-day cumulative cap. The "walk the whole table 1000 rows at a time until exhausted" pattern is stopped by the cumulative caps.

**Confirmation status.** The **posture** — CAPPED, ban bulk-collapse + unbounded pagination — is **[OPERATOR-CONFIRMED C]**. The **numeric values** above are the pinned proposed defaults, to be confirmed against real query patterns before build.

---

## §4 — AST allowlist + restricted role + read-only txn (SELECT-parsing is NOT the boundary)

**Principle:** "it parses as a SELECT" is NOT a security boundary. Three independent layers: (a) a version-matched recursive AST allowlist against an **exact manifest**, (b) a hardened DB role, (c) a governed read-only transaction. None is trusted alone.

### 4a — Recursive AST allowlist (PostgreSQL-version-matched) + EXACT manifest

Parse with a parser matching R's exact Postgres major version. Walk the **entire** tree recursively (sub-selects, CTEs, expressions, function args). **`search_path` is locked** to the `live` schema; **CI + boot + swap-time fail closed on schema drift** (the running **schema-fingerprint** must equal the manifest-signed fingerprint).

**Pinning is by NAME + schema-fingerprint, NOT by build-time OID (blocker 8).** OIDs change on every `pg_restore`, so a build-time OID pin cannot survive this restore design. Instead:

- The **relation/function manifest is a reviewed FILE artifact** checked into the repo (`manifests/relation-manifest.json`, hash-pinned), enumerating every allowed relation by **schema-qualified NAME** and every allowed function/operator/cast by **schema-qualified name + argument-type signature** — **not** a runtime enumeration placeholder. New/renamed relations/columns/functions are **default-DENIED until reviewed** (even though the product goal is entire-current-DB access).
- The **schema-fingerprint** = `SHA-256` over the canonically-ordered set `{schema.table.column:pg_type}` (relations sorted by schema-qualified name, columns by ordinal). It is bound into the signed manifest (§ Signed manifest) and **verified at BOTH swap-time and boot**: R's actual fingerprint must equal the pinned/signed value or the system **fails closed**.
- **Generation-bound OID resolution.** After each verified import, P resolves the manifest's names → the **OIDs of THAT accepted generation** into a generation-bound runtime table (name→OID valid only for generation N). The AST allowlist checks names against the file manifest; the executor uses the generation-bound OIDs. **Runtime OIDs are never compared to build-time OIDs.** Any name present in the manifest but absent/mismatched in the live schema → **fail closed**.

**Relation manifest contents:**

- **Auth/credential relations (BetterAuth — INCLUDED per §2):** `public.user`, `public.session`, `public.account`, `public.verification`. (These carry `session.token`, `account.access_token/refresh_token/id_token/password`, `verification.value` — in-scope by explicit operator override, §2.)
- **Incluir application relations:** the full set of base tables/views in schema `public` at snapshot time (student/guardian/enrollment/health/support/financial/document/staff/course/class/attendance and all others), **each pinned by schema-qualified name** in the reviewed file artifact. *The exact app-table set is **pending operator confirmation** — the enumeration mechanism (from the verified snapshot's `information_schema`), the reviewed-file-artifact form, the name+fingerprint pinning, and default-deny-on-new are all FIXED; only the concrete name list awaits the operator's confirmation of which app tables exist.*

**Exact safe-function/operator/cast manifest (reviewed file artifact), pinned by schema-qualified name + argument-type signature:**

- **Allowed:** comparison + boolean operators; arithmetic; `lower`, `upper`, `length`, `substring`, `trim`, `coalesce`, `nullif`; date filters `now`, `date_trunc`, `age`, `date_part`; text/int/date casts; scalar bounded aggregates `count`, `min`, `max`, `sum`, `avg` (over `LIMIT`-bounded input only).
- **Everything else DENIED by default** (including the bulk-collapse aggregate set, §3).

**DENY (non-exhaustive, all recursive):**

- **DML in CTEs** — `WITH ... (INSERT/UPDATE/DELETE/MERGE ...)`; any data-modifying CTE.
- **`SELECT INTO`** (creates a table).
- **Locking clauses** — `FOR UPDATE`, `FOR NO KEY UPDATE`, `FOR SHARE`, `FOR KEY SHARE`.
- **Recursive/`WITH RECURSIVE`** and **set-returning bombs** — unbounded `generate_series`, cartesian-explosion joins (caught by the mandatory plan-cost ceiling, §4c/§3).
- **`COPY`, `CALL`, `DO`** (procedural / bulk I/O).
- **System catalogs** — `pg_catalog.*`, `information_schema.*`, `pg_*` relations (schema/credential reconnaissance).
- **Foreign tables** / foreign-data-wrapper relations.
- **`SECURITY DEFINER` functions, UDFs, volatile functions**, and any function touching: **filesystem** (`pg_read_file`, `pg_ls_dir`, `lo_import/lo_export`), **network** (`dblink`, FDW connect), **large objects**, **config** (`set_config`, `current_setting` write form, `pg_reload_conf`), **`pg_sleep`/sleep**, **advisory/lock** functions, and **dynamic SQL** (`query_to_xml`, any exec-string form).

Deny-by-default: anything not explicitly on the relation/function manifests is refused before dispatch.

### 4b — Broker DB roles (structural backstop)

Two roles on R, both minimal:

- **Query role** (broker → E7): **`NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS`**; **no role memberships**; **no `CREATE`, no `TEMP`** on any database/schema; **revoke default `PUBLIC EXECUTE`** on functions, grant `EXECUTE` only on the audited safe-function manifest; **explicit `SELECT` grants only** on exactly the allowlisted relations — **no blanket schema `SELECT`**, **no write privilege anywhere** (query-role write-denial is proven in evidence D).
- **Importer / maintenance role `snap_admin`** (P importer → E8): may create/restore into the **`staging` schema** (on the per-generation tablespace) and perform the swap DDL (the single-txn schema rename, § Atomic swap); **separate from the query role**; the query role has `search_path` locked to `live` and can never see `staging`.

### 4c — Governed READ ONLY transaction

Every query runs inside `BEGIN TRANSACTION READ ONLY; ... ROLLBACK;` with, pinned at the role/session:

- `statement_timeout = 5s`, `lock_timeout = 1s`, `idle_in_transaction_session_timeout = 10s` (all small, pinned).
- **`work_mem = 8MB`** and **`temp_file_limit = 64MB`** (starve sort/hash bombs; cap spill).
- **Connection limit** on the role + **concurrency 1** (broker serializes; one in-flight query).
- **Mandatory plan-cost ceiling = 1,000,000** — reject before execution if `EXPLAIN` estimated total cost exceeds the bound (catches cartesian explosions + whole-table aggregate scans the row cap would otherwise stream into; an outer `LIMIT` does NOT bound these — §3). **Not optional.**
- **Server-side cursor, FETCH chunk = 50** — oversized results are cancelled **before materialization** (§3).

---

## §5 — Fixed query broker

**Split of privilege:** Hermes (U) holds **NEITHER DB credentials NOR a network route to R**.

- **Broker (in P) is the sole credential + route holder.** The broker is the ONLY component holding short-lived **TLS + SCRAM** R credentials and the ONLY component with a network path to `R:5432` (§6 segments enforce this).
- **Broker owns the boundary logic.** The AST parser + manifest (§4a), the caps engine (§3), the READ ONLY transaction governance (§4c), the `as_of`/generation/freshness check (§ Freshness), the **capability verification** (§11), and the audit emission (§7) all live in the broker — NOT in U, NOT in the model.
- **Fixed interface.** U calls the broker over a fixed internal mTLS RPC (E1) with a single operation ("run this capability + exact read-only SQL bytes"). The broker exposes no other operation, no passthrough, no admin surface.
- **DB results are untrusted DATA, never instructions.** Result strings are treated as data; the broker never interprets DB output as control and never lets DB content re-enter as a new instruction. **Cap tool loops** — the broker enforces the per-event cumulative cap (§3) and the model has no autonomous follow-up loop (§14).
- **Sanitize DB errors.** DB error text is sanitized before it reaches the model (strip literals, connection strings, schema internals, credential fragments). The model sees a normalized error class, not raw Postgres output.

---

## §6 — Executable segmented network graph (on-box U/P/R)

**No public attack surface; internal segmentation is executable, not aspirational.** This replaces any cloud-firewall/cloud-IAM prose — the enforcement is the U/P/R VM topology (§1) on the Mini, with a PF outer backstop.

- **No public inbound to U or R.** Neither U nor R has any external attachment. Only **P** has an external attachment, and on it only **E4/E5/E6 outbound** (OpenAI, Slack, audit sink) and **E9 inbound** (source snapshot push) are permitted.
- **Admin plane U cannot reach.** Administration is via local console or a separately-authenticated management plane; **no SSH port is open to U**, and U cannot reach the admin plane.
- **Executable ACLs (the directed-edge graph, §1a):**
  - **U** may reach ONLY P's broker RPC, OpenAI gateway, and Slack relay (E1/E2/E3) — nothing else. U has **no external DNS and no default route**.
  - **P (broker/importer) ALONE** may reach **R** (E7/E8). U cannot reach R; nothing external can reach R.
  - **ONLY P** resolves external DNS and holds a default route (for E4/E5/E6). **Generic IP forwarding on P is disabled** — P is not a router/NAT for U.
  - **R** resolves no external DNS, holds no default route, and **initiates nothing**.
- **Two firewall layers + PF backstop:** per-VM guest firewalls (U, P, R) **plus** the host-level **PF anchor as outer backstop + boot interlock** (§1). Defense-in-depth; the topology — not PF — is the primary boundary.
- **Egress proxy discipline (P external, carried from narrow lane):** listener-keyed ACLs; **CONNECT/TLS 443 only**; **SNI/authority allowlist** (`api.openai.com`, Slack Socket Mode host family, the audit-sink FQDN); **DNS-rebinding checks** (re-resolve and reject at dial time; reject if the resolved IP is not in the allowed set); deny **IPv4 + IPv6** private ranges (RFC1918 / CGNAT 100.64/10 / ULA fc00::/7 / loopback / link-local 169.254/16 + fe80::/10), **cloud/host metadata**, **multicast**, **UDP**, and **raw-IP dials** (no host without an allowlisted SNI).
- **Negative + positive probes (part of go-live evidence A/F):**
  - **Wrong-listener** — U dialing R:5432, or the broker dialing OpenAI, is refused.
  - **Reverse-reach** — R cannot reach out to anything; P cannot reach the source/prod; U cannot reach any external host.
  - **Positive** — U→broker succeeds, U→R-direct fails; broker→R succeeds; U→OpenAI-gateway→pinned path succeeds, U→api.openai.com-direct fails; Slack only via P's proxy.

---

## §7 — PII-safe, tamper-resistant audit (external mTLS append-only + keyed HMAC)

**Do NOT log raw SQL by default** — literals contain names, CPF, DOB, AND (per §2) session tokens, OAuth tokens, passwords, verification tokens. Raw SQL text is itself a credential-exfil channel.

**Audit sink & route (blocker 11 — identity structure pinned).** P's audit client ships records to a **fixed, external, append-only WORM/object-lock sink over mTLS** (edge **E6**), reachable ONLY from P over that one mTLS route. Pinned structure (exact identity values marked **pending operator confirmation**, but the shape is fixed and load-bearing):

- **Sink identity:** a single pinned **FQDN/service** (`audit-sink.PENDING`), an **object-lock store in COMPLIANCE mode** (immutable, not GOVERNANCE — no privileged override of the lock).
- **Retention window:** pinned **≥ 400 days** (default; operator-confirmable) — long enough to outlive an incident-detection window.
- **Append-only identity:** a dedicated **mTLS client identity** with an IAM policy granting **`PutObject` + object-lock-retain only — no `Get`/`Delete`/`Overwrite`/lock-release**. (Write/append only; a compromised P cannot read back or rewrite its own trail.)
- **Key rotation:** the mTLS client key + the audit HMAC key rotate on a **pinned 90-day schedule**, each rotation proven old-rejected/new-accepted (§8).
- **Route:** this is a fourth allowlisted external destination alongside OpenAI + Slack + E9-ingress-return, with **no general egress**. The sink lives **outside all three VMs**, so a compromised VM cannot rewrite its own audit trail.

**Every query emits an audit record containing:**

- **Keyed HMAC of the exact raw query bytes** — **`HMAC-SHA-256(exact_sql_bytes, audit_key)`** with a **rotated audit key** (in P tmpfs, §8) — **NOT a merely salted hash** — so identical queries correlate without storing text and the value is not offline-recomputable without the key.
- **Literal-stripped normalized AST fingerprint** — the query's structure with all literals removed/parameterized (two queries differing only in a CPF value share a fingerprint).
- **Referenced tables + columns** (from the AST — reveals which sensitive/credential columns were touched).
- **Trusted Slack event/user correlation** — the Slack event id + authenticated user id, bound out-of-band from the Slack event context (§11), NEVER a model argument.
- **Result count, result bytes, duration, outcome** (served / denied-by-AST / capped / stale-refused / error-class).
- **`as_of` + snapshot age + generation** (§ Freshness) — **not streaming lag.**

**Durability + fail-closed (two-phase):**

- **Durable INTENT appended BEFORE execution.** The audit intent record (actor, HMAC, fingerprint, tables/columns, caps-remaining, `as_of`/generation) is appended to the external sink **before** the query runs. **Pre-intent append failure blocks execution** — no query runs un-audited.
- **Terminal OUTCOME appended AFTER.** The outcome record is appended after execution. **Outcome-append failure withholds the result** from the model/Slack while the durable intent record remains (so the attempt is never invisible).
- **Retention/access/key rotation pinned** above (400-day retention, COMPLIANCE object-lock, 90-day key rotation) — not left "unnamed" as in v2.

**Raw-SQL retention requires a NEW security review (blocker 11) — it is NOT a runtime opt-in.** Because SQL literals carry names/CPF/DOB AND (per §2) session tokens, OAuth tokens, passwords, and verification tokens, raw-SQL text is itself a credential-exfil channel. It is **off by default and cannot be enabled at runtime**; enabling any raw-SQL retention requires a **fresh Cipher security review** of a SEPARATE encrypted, access-restricted, append-only store with an explicit short retention window (never mixed into the normal audit stream, never in plaintext logs).

---

## §8 — Secrets via tmpfs / workload identity [OPERATOR-CONFIRMED F: dedicated key]

**No secret ever touches an image, compose file, git, env var, argv, or stdout.**

- **Injection surface:** credentials are delivered via **tmpfs mount** (memory-backed, never on disk) or **workload identity** (short-lived). Never baked into an image, never in `docker-compose`/env/argv, never echoed to stdout/logs.
- **Key custody by VM (complete list, all in P tmpfs only — blocker 4):** the **snapshot `age` X25519 decrypt private key**, the **audit HMAC key**, the **capability-signing key** (§11), the **R-DB SCRAM credential**, the **source-push mTLS trust anchor** (to validate the source client cert), the **E1 U↔P mTLS server key**, and the **OpenAI + Slack tokens** all live **ONLY in P's tmpfs**. U holds no key; R retains no decrypt key (R's data is decrypted in-stream by P and lands only inside R's encrypted Postgres storage).
- **U (Hermes) gets NO real credential.** U holds no real OpenAI key (a dummy/placeholder only; the real key lives in P's OpenAI gateway) and no DB credential (the broker in P holds it, §5).
- **Dedicated project-scoped OpenAI key [OPERATOR-CONFIRMED F].** P's OpenAI gateway uses a **dedicated, project-scoped** OpenAI API key issued for this workload only — NOT the shared General key. This scopes blast radius + quota + retention posture (§10).
- **Rotation with proof.** Each credential — **R DB (SCRAM), OpenAI, Slack, source-push mTLS trust anchor, source Ed25519 signing key (source-side), audit-sink mTLS + audit HMAC key, capability-signing key, E1 U↔P mTLS keys** — is rotatable, and rotation is proven with an **old-rejected + new-accepted** test. Rotation drills are part of go-live evidence.

### §8a — Cold-boot secret bootstrap + R-volume unlock (blocker 4)

**Posture: no durable plaintext key on the box.** After a Mac **cold boot** there is no plaintext key material anywhere; nothing auto-unlocks. Bootstrap is by **operator-provided unlock at boot** — the **default design, marked "proposed = operator-unlock-at-boot, pending operator confirmation."**

- **What the operator provides.** At cold boot, the boot-orchestrator (§1b, gate G1/G2) prompts on the **local console / separately-authenticated management plane U cannot reach** for the **operator unlock passphrase**. This is the ONLY inbound secret; it is entered once per cold boot.
- **What it unwraps.** Via a pinned KDF (Argon2id, pinned params) the passphrase derives a wrapping key that unwraps two sealed blobs held **wrapped-at-rest** on P's encrypted disk (never plaintext at rest):
  1. **R's volume encryption key** → unlocks R's encrypted DB/WAL/temp/base volume (gate G1) — R is booted only after unlock succeeds.
  2. **P's sealed key bundle** → the full §8 custody list (age decrypt key, audit HMAC, capability-signing key, R-DB SCRAM, source-push mTLS trust, E1 mTLS server key, OpenAI/Slack tokens) → materialized **into P's tmpfs only** (gate G2).
- **No secret-store edge needed (resolves blocker 4's edge-graph gap).** Because bootstrap is authenticated manual-console injection, **no network secret-store egress edge is added** to the directed-edge graph (§1a) — the §1/§51 "no host integration / no extra egress" posture is preserved. There is no `Infisical`/tailnet secret pull at boot.
- **Per-generation DEKs** (the ingest-landing DEK and each `snap_gen_N` DEK) are likewise sealed under the operator-unlock key material and live only in memory once unwrapped; destroying a generation's DEK (crypto-erase, § Atomic swap) is destroying that in-memory + sealed-blob key.
- **Alternative (noted, not chosen):** macOS **Keychain / Secure Enclave auto-unlock** with the key bound to the Mac (no operator prompt at boot). This trades the operator-presence requirement for a key that lives (Secure-Enclave-bound) on the box. **The choice is "proposed = operator-unlock-at-boot, pending operator confirmation"** — operator-unlock fits the stated "no durable plaintext key on the box" posture and is the default; the Enclave alternative is recorded for the operator to choose.

*(No secret VALUES appear in this spec; all secrets are referenced by role and sourced from the pinned sealed store / operator unlock. No `Infisical`/network secret edge exists in the bootstrap path.)*

---

## §9 — OpenAI ceiling: 50K tokens/UTC-day [OPERATOR-CONFIRMED F, NOT 150K]

**The token cap is COST control, NOT privacy control.** Data-exfil rate is governed separately by the bulk caps (§3). This section only bounds spend.

**Enforced in the OpenAI gateway (in P, structurally unavoidable, §6):**

- **Fixed model snapshot + path:** `gpt-4o-mini-2024-07-18`, `POST /v1/chat/completions`, **non-streaming, text-only**, **`store=false` forced** in the reconstructed body. The gateway reconstructs an allowlisted upstream body from scratch and drops any caller-supplied model / URL / auth / metadata / files / images / audio / extra tools / `store` override.
- **Pinned tokenizer:** **tiktoken `o200k_base`**, counted over the **full serialized request** (system + all messages + tool schema), not just the user turn.
- **Bounds (PINNED values):** **max input tokens = 24,000** (generous enough to carry a full 64 KB DB result + system prompt; note the per-UTC-day 50K-token ceiling binds first on large results — acceptable, it is cost control), **max output tokens = 1,500**, **max messages = 20**. A request exceeding any bound is rejected pre-dispatch. These fit both the §3 64 KB result-byte cap and the 50K-token daily ceiling.
- **Concurrency 1.**
- **Atomic reserve + authoritative reconcile:** before dispatch, atomically reserve `tokenized_input + max_output` against the day's budget; after response, reconcile against the returned `usage`. **Kill-closed** (reject, do not queue) on breach.
- **Durable counters:** persist across gateway/VM restart; reset atomically on the **UTC-day** boundary. **Fail closed** if `usage` is missing OR the counter store is unavailable.
- **Ceiling value [OPERATOR-CONFIRMED F]:** **50,000 tokens/UTC-day**. Raise only from observed legitimate usage, operator-approved.

---

## §10 — Retention + processor posture [OPERATOR-CONFIRMED D]

**`store=false` does NOT eliminate OpenAI's abuse-monitoring retention.** Setting `store=false` prevents OpenAI's persistent storage for training/history but does not by itself remove the abuse-monitoring copy (default ~30 days).

- **Pursue ZDR/MAM [OPERATOR-CONFIRMED D].** For the dedicated OpenAI project (§8), pursue **Zero Data Retention** / no-model-training (**ZDR/MAM**, enterprise or API-agreement). If ZDR/MAM is **unattainable**, the operator **explicitly accepts** the documented **~30-day default abuse-monitoring retention** — credential + full-PII payloads may sit in OpenAI's abuse-monitoring store for up to ~30 days. This compounds the §2 account-takeover acceptance.
- **Slack retention [OPERATOR-CONFIRMED D].** Pin the Slack DM retention/deletion policy for the operator DM channel. Per the operator's blanket confirmation: if a tighter Slack deletion policy is not configured before go-live, DEFAULT Slack retention is EXPLICITLY risk-accepted.
- **Local posture on the VMs:** **no local prompts / results / query history**, **no swap**, **no core dumps**, **encrypted disk** (all three VMs). Nothing sensitive lands on persistent local storage outside R's encrypted Postgres storage.
- **Legal posture.** DPA / lawful-purpose treatment for **minors** + **health/support** + **financial** data is **elevated**, because credential material and full PII both cross the boundary AND may be retained (OpenAI abuse-monitoring, Slack). Documented as heightened LGPD/DPA exposure alongside the §2 decision.

---

## §11 — Slack actor authenticity + one-use capability (Cipher blockers 6 & 7)

**Authorization has NO fallback.** The prior `service_actor=hermes` fallback is **REMOVED entirely** — it cannot authorize a query.

- **Trusted Slack context REQUIRED.** The operator identity is trusted **only** from the authenticated Slack event that arrives through **P's Slack proxy** (which owns the Socket Mode connection + tokens), out-of-band from any model argument. **If trusted Slack context is not present, the query is REFUSED.** There is no path by which the model asserts who the actor is, and no service-actor substitute.
- **Durable capability/counter store (PINNED).** Nonces AND the §3/§9 cumulative counters live in a **P-local durable, key-sealed KV store** on P's encrypted disk (its keys sealed under the §8a operator-unlock material), **fsync-durable across P/broker restart**. It is a P-internal store — **no new edge** in §1a. **All caps/nonce operations fail closed if the store is unavailable.**
- **One-use capability minting (P).** After P verifies (a) the Slack event authenticity (received on P's authenticated Socket Mode connection) and (b) `user id == U050U6BNCS0` allowlist, P's capability minter (using the tmpfs capability-signing key, §8) issues a **one-use, short-lived capability** — a signed token **bound to:** `{ slack_event_id, slack_user_id, sha256(exact_sql_bytes), nonce, expiry, remaining_caps }`. **Expiry = 120s (PINNED).**
- **Atomic nonce consumption (blocker 9).** The nonce is recorded with an **atomic `SET-NX`-equivalent** in the durable store, **TTL = 180s (PINNED, ≥ the 120s validity so a valid capability can never outlive its nonce record; mirrors the narrow-lane 180s replay window)**. The record persists across P/broker restart. **Concurrent same-nonce use is rejected** (the first `SET-NX` wins; the second sees the key present → refused). Store outage → **fail closed** (no consumption possible → no query).
- **Broker verifies the capability (no fallback).** The broker executes a query ONLY if a valid capability accompanies it: verify the signature, confirm the **nonce is unused** (atomic `SET-NX` succeeds), **not expired** (≤120s), and **`sha256(presented_sql_bytes)` matches the bound hash exactly**. Any mismatch/replay/absence/store-outage → **refused**. Cryptographic end-to-end binding from Slack event → exact SQL bytes → execution.
- **Direct SQL — the model must not rewrite the bytes.** On the direct-SQL primary path (§14), the operator's SQL bytes from the Slack event are the exact bytes the capability binds and the broker runs. The model does not rewrite them.
- **NL→SQL.** Displaying the generated SQL and receiving a **fresh authenticated operator confirmation** mints the one-use capability for **exactly that confirmed SQL**; altered or replayed SQL fails the hash/nonce check and is rejected.
- **Deduplicate Slack events** (Slack redelivers; the broker must not double-execute — the nonce/one-use capability also enforces this).
- **Denied user = ZERO downstream.** A non-allowlisted user (anyone but `U050U6BNCS0`, plus all bots) produces **ZERO LLM calls, ZERO capability mint, ZERO broker calls, ZERO DB queries** — rejected at P's request layer before any model/tool/DB work. Proven with request-count evidence.

---

## §12 — Runtime tool-absence proof + hash-pinned manifest + CVE gate (Cipher blocker 10)

**Prove the effective tool inventory is EXACTLY `{chat, generic-SQL-read}` and nothing else — at runtime, not just in config.**

- **Hash-pinned tool manifest (actual identities, not labels — blocker 11).** The manifest hash-pins the **actual registered tool IDs / module paths / plugin identifiers** the runtime reports (not the conceptual labels "chat"/"SQL tool") to exactly the two intended tools; the concrete hash is **pinned at build against the pinned image** (`hermes` at its pinned digest). Boot fails closed if the runtime tool set's hash does not match the pinned manifest.
- **Invoke-and-reject (open-world negative probes).** For every known capability path — shell, filesystem, browser, **raw HTTP/fetch/socket**, **alternate DB clients**, **language interpreters**, **arbitrary/dynamic tool registration**, code-exec, cron, delegation/subagents, MCP (beyond the one query tool), hooks, plugins, installers, memory providers, **and every upstream Hermes module** — attempt to invoke it and show it is **rejected BEFORE the handler runs**, with **zero child-process spawn, zero file write, zero network call, zero state delta**. Rejection must be **persistent across restart**.
- **Pinned image + CVE gate (scanner identity pinned — blocker 11).** Pinned image **digest + version**; **SBOM + CVE scan at pin time** using a **pinned scanner name + version + vulnerability-DB snapshot timestamp** (default: **Trivy, pinned version, with the vuln-DB snapshot recorded by timestamp** — pending operator confirmation of the exact scanner build). **Fail closed if scanning is unavailable** (no scan → no pin). **CVE gate:** **no exploitable critical/high finding** may ship **unless explicitly operator-accepted with a named owner and an expiry date**. The pin is rejected otherwise.
- **Memory-provider-bypass advisory** (affects versions **≤ 0.16.0**): reconcile against the pinned image and prove the bypass path is **absent** (record `hermes --version`; reject the image if the affected memory/plugin bypass path is present).

---

## §13 — Lifecycle / incident response

- **Base + provisioning:** minimal **LTS arm64** base images, **IaC-provisioned** (reproducible), **Secure Boot / verified boot** where supported, **encrypted disk** (FileVault + guest disk encryption), **no swap / no core dumps**.
- **Patching:** critical-patch SLA **24–48h**; routine monthly patch cycle; **drift detection + boot attestation** (the running VMs match the IaC-declared, attested state; boot interlock, §1).
- **Backups — no secondary copies.** **NO snapshot/backups of R by default.** The ONLY sanctioned copy of the accepted PII+credential dataset is **the current encrypted snapshot on R**, plus **bounded `staging` / `old_gen_N` schema material during a swap window** (§ Atomic swap). Any other backup that is ever created must be **separately encrypted + retention-bound** — never an unencrypted, unbounded copy of a DB that now contains credentials in scope (§2).
- **Kill switch:** an **INDEPENDENT outer kill switch** (cut P's external egress at the PF backstop / host layer, independent of anything inside the VMs) **+ VM stop**. Cutting egress halts all OpenAI/Slack/audit/ingest traffic even if a guest is compromised.
- **System-credential revocation on incident:** revoke Slack / OpenAI / R-DB / source-push / audit-sink credentials.
- **USER-credential-compromise playbook (Cipher blocker 11) — MANDATORY because §2 deliberately copies live user credentials.** Risk acceptance does NOT remove containment duties. On suspected compromise of R / broker / LLM path / Slack channel:
  1. **Invalidate ALL Incluir sessions** (delete/expire every `session` row; force global re-login).
  2. **Invalidate ALL verification / reset tokens** (`verification.value`) so recovery flows can't be replayed.
  3. **Revoke OAuth access + refresh tokens provider-side** where possible (`account.access_token`/`refresh_token`/`id_token`) — refresh tokens survive local rotation, so provider-side revocation is required.
  4. **Rotate affected signing secrets** (BetterAuth secret + any signing-key columns present).
  5. **Force password reset / credential migration** as applicable (`account.password`).
  6. **Trigger the minors / health / financial LGPD notification + evidence path** (data-subject + authority notification as required; preserve evidence).
  This user-credential playbook is **in addition to** system-key revocation above.
- **Clean rebuild:** after any compromise, **rebuild clean from IaC** (do not patch a compromised host in place); destroy + re-push R's snapshot (§ Deletion/rebuild); produce **deletion/crypto-erase evidence** for any sensitive data.
- **Alerts:** denied users, audit gaps (append failures), cap hits (§3), **push/verify failures + snapshot age ≥ 24h + stale-refusal at 26h** (§ Freshness — NOT replica lag), egress anomalies, auth failures.
- **Pre-live drills:** **kill-switch drill** + **credential-rotation drill** + **cadence/stale-refusal drill** executed and evidenced before go-live.

---

## §14 — SQL-direct vs NL [OPERATOR-CONFIRMED E = direct SQL primary]

- **Direct SQL is the transparent primary interface [OPERATOR-CONFIRMED E].** The operator types read-only SQL; the broker executes **exactly the audited bytes** (§7) bound by the one-use capability (§11). **No model-in-the-loop translation on the primary path** — what is audited is what ran, and the model does not rewrite the bytes.
- **If NL→SQL is enabled** (opt-in, not the default): the tool **MUST display the exact generated SQL + an estimated scope** (estimated rows/cost, tables/columns touched) and require a **NEW authenticated operator confirmation** (§11 signed event) **BEFORE execution**. That fresh confirmation mints the one-use capability for exactly the displayed SQL; the broker runs and audits exactly those bytes.
- **NO autonomous follow-up query loops.** The model may not chain queries on its own. One confirmed query per confirmation; any further query needs a fresh operator turn (reinforces the §3 per-event cumulative cap).

---

## §15 — VM host [OPERATOR-CONFIRMED A — ON-BOX true-VM U/P/R]

**[OPERATOR-CONFIRMED A]** The host is the **Mac Mini, on-box**, running the three VMs (U/P/R, §1) under a **real hypervisor** — **Apple Virtualization.framework / UTM**, genuine own-kernel VMs. **NOT** OrbStack (rejected by Cipher: OrbStack machines share one kernel; `--isolated` is not a kernel-escape boundary). **NOT** a paid cloud VM. **NOT** a streaming replica.

- **Operator-accepted residual:** the operator **explicitly and knowingly accepts** the **shared-host hypervisor-escape residual** — a hypervisor escape lands beside prod-adjacent incluir-postgres/watchtower on the Mini. His framing: *"security risks are life, minimize them as much as possible"* — residual-accepted does **not** mean skimp; isolation is built as durably as reasonably achievable (fail-closed, restart-persistent, not brittle ad-hoc firewall rules).
- **Replica-host ruling honored (Cipher, 2026-07-30):** the snapshot node **R must NOT share a host/kernel/storage failure domain with the prod primary**. It does not: R is its own VM holding a **one-way pushed snapshot**, with **no reverse route to xerox**, never co-resident with the prod primary and never reachable from prod/tailnet. This is the sanctioned alternative to a same-host streaming replica (which Cipher rejected).
- **Durable enforcement:** primary boundary = the U/P/R VM topology + per-VM guest firewalls; **PF anchor = outer backstop + boot interlock only** (§1, §6); **no auto-fallback to shared/NAT networking**; policy attested at boot before U/R start.

---

## § Go-live evidence set — SNAPSHOT A–F (Cipher blocker 12)

Go-live requires a Cipher **PASS** against ALL of the following. Evidence is config-level + probe-count + drill artifacts — never a single passing query. **These are Cipher's snapshot-specific A–F items; they REPLACE the prior streaming-standby go-live items (no `pg_is_in_recovery`, no replica-lag).** Each suite is run **initially, and after: P restart, U restart, R restart, hypervisor restart, Tailscale restart/renumber, and full Mac cold boot.**

**A. U/P/R isolation + boot interlock + secret bootstrap.** Own-kernel VM + config proof for U, P, R; the exact **vNIC / listener / route matrix**; the full **negative + positive** network suite; the **§1b `launchd` boot-orchestrator** running the pinned G0→G3 sequence, with each **disable-one-precondition drill (PF, vmnet identity, R unlock/health, P attestation) leaving U stopped** — **never** NAT/shared fallback; U/R **autostart disabled**; **§8a operator-unlock-at-boot** proven (no auto-unlock, no durable plaintext key at rest, no secret-store edge).

**B. Authenticated push (terminating ingress).** Source-to-ingest push proof **plus reverse-initiation denial** (R/prod cannot be dialed from the wrong side; source-initiated only); **P terminates, does not forward**; **no shell/command surface**; **mTLS client-cert pin + source-IP allowlist**; **pre-slot handshake/idle/rate bounds** proven to defeat slow-hold of the single ingest slot; **confined non-root receiver** (no broker-socket/tmpfs-secret access, dedicated encrypted landing volume); **512 MB / 15 min / concurrency-1 bounds** enforced; **`age` X25519 decrypt + Ed25519 manifest signature + ciphertext-digest parity** (verify-then-decrypt, TOCTOU-free); **manifest ↔ source-identity ↔ `as_of` ↔ `generation` ↔ schema-fingerprint binding**; and **replay / rollback (below P's durable floor) / future-time (>±120s) rejection**.

**C. Key & plaintext.** All keys (§8 list) present **only in P tmpfs**, **absent from U and from durable R state**; **no plaintext archive dump at rest** anywhere (ciphertext landing file only, decrypt-in-memory); **R base DB/WAL/temp volumes encrypted** + **each generation's table data on its own per-generation encrypted virtual disk/DEK**.

**D. Stage/import/swap (single-txn schema rename + per-gen crypto-erase).** The **crash matrix** (crash before COMMIT → `live` intact via crash-recovery rollback; crash after COMMIT before old-erase → new `live` correct + boot-reconcile completes crypto-erase); **failed in-txn canary probe → ROLLBACK restores old `live` before service**; **schema-fingerprint + row-count parity** on success; **broker drain + `snap_admin`/query-role separation** and **query-role write-denial** proven; **`DROP SCHEMA old_gen CASCADE` + per-generation DEK destruction** = selective crypto-erase (with the precisely-stated shared-WAL residual).

**E. Cadence / max-age.** Failure drills: **push-failure alerts**; **missing/invalid/stale/below-floor manifest refuses BEFORE any SQL** (fail-closed at 26h, and on rollback vs P's durable floor); **`as_of` + age + generation shown end-to-end** (answer surface + audit) across normal and stale conditions.

**F. Source export + reach + kill.** **Source-export SLO proof** (CPU/I-O/bandwidth caps, ≤5% prod p95 delta, load-guard, DDL-exclusion, off-peak window — measured, not asserted; a stuck receiver cannot block prod); **R-only DB reach from P** (P→R succeeds), **U→R-direct denial**; the **exact audit-sink mTLS path** proven (append-only WORM/object-lock, no general egress); and **kill-switch / rebuild / deletion (per-gen crypto-erase) proof**.

*Cross-cutting proofs folded into A–F:* tool-absence hash-pin (actual tool IDs) + open-world negative probes + CVE gate with pinned scanner/vuln-DB (§12); denied-user zero-request proof (§11); cap enforcement with **atomic reserve-then-reconcile + mid-stream cancel+rollback + outer-LIMIT-rejection** (§3); OpenAI ceiling atomic-reserve/reconcile + durable counters + UTC reset + `store=false` (§9); **atomic SET-NX nonce (TTL 180s) + durable cap store, fail-closed on outage, restart mid-query/mid-stream tested** (§3/§11); **durable P-side generation floor survives R rebuild** (§ Durable generation floor); audit fail-closed two-phase (intent-before / outcome-after) with keyed HMAC over the external mTLS append-only route (§7); rotation drills for every credential incl. source-push + audit + capability + E1 mTLS keys (§8).

---

*End of v2 spec. DESIGN DOCUMENT ONLY — no build/deploy clearance. Single mechanism: on-box true-VM U/P/R pushed-snapshot. Decisions A/C/D/E/F are OPERATOR-CONFIRMED (2026-07-30, blanket "just go" via GLaDOS); go-live still awaits a separate Cipher PASS against the § Go-live evidence set (snapshot A–F) at a fresh exact hash.*
