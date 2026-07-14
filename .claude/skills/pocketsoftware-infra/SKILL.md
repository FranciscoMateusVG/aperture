---
name: pocketsoftware-infra
description: PocketSoftware production server operations — the second OCI server (separate from xerox), its Terraform repo, Dokploy org, Tailscale-gated access, backups, and safety tiers. Use for ANY operation touching the pocketsoftware server, test.pocketsoftware.com.br or *.pocketsoftware.com.br deploys, the pocketsoftware-terraform repo, its Dokploy org, or its remote Terraform state. Triggers on "pocketsoftware", "pocketsoftware.com.br", "163.176.231.29", "pocketsoftware-terraform", "pocketsoftware compartment", "resize pocketsoftware", "pocketsoftware deploy", "pocketsoftware backup".
---

# PocketSoftware Production Server

The production-lane OCI server for PocketSoftware. **This is NOT a fool-around box** — xerox is the experimentation server; this one carries production discipline. Built 2026-07-14 (BEADS `aperture-5szli`, spec + plan in the repo's `docs/`).

---

## 1. Server Identity

| Field | Value |
|---|---|
| Cloud | OCI, region `sa-saopaulo-1`, compartment `pocketsoftware` (tenancy shared with xerox — compartment-isolated) |
| Shape | `VM.Standard.A1.Flex` — **6 OCPU / 36GB RAM / 300GB boot**, Ubuntu 24.04 ARM (aarch64) |
| Reserved public IP | `163.176.231.29` (permanent — survives instance rebuilds) |
| Tailscale | hostname `pocketsoftware`, IP `100.102.73.112`, Tailscale SSH enabled |
| SSH (break-glass) | `ssh -i ~/projects/pocketsoftware-terraform/keys/pocketsoftware-ssh ubuntu@163.176.231.29` |
| Repo | `~/projects/pocketsoftware-terraform` (branch `main`) |
| DNS | `pocketsoftware.com.br` apex + wildcard `*` → 163.176.231.29 (any subdomain resolves, no DNS trips needed for new apps) |
| Cost | ~$90/mo (instance + backups + state bucket) |

**Resize procedure** (within A1.Flex family): edit `ocpus` / `memory_in_gbs` in `compute.tf` `shape_config` → operator-approved `just apply` → in-place resize, ~3 min reboot, same disk + IP. Storage grows online (expand volume in OCI, grow fs). Changing to x86 = full rebuild (don't).

## 2. Safety Tiers — same as xerox, enforced harder

| Tier | Operations | Rule |
|---|---|---|
| Read-only | `just remote-status`, `remote-ps`, `remote-logs`, Dokploy GET endpoints, `terraform plan` | Run freely |
| Mutative | `just apply`, `remote-exec`, `ansible`, Dokploy create/deploy/update/stop/start | **Operator approval first** |
| PROHIBITED | Any delete — compose services, projects, databases, volumes, `terraform destroy` | **Never. No exceptions.** |

`terraform apply` is ALWAYS operator-gated, no exceptions — state is remote but applies bill money and mutate prod.

## 3. Terraform Remote State + THE checksum gotcha

- Backend: S3-compat → bucket `pocketsoftware-tfstate` (**versioned**, tenancy ROOT compartment), namespace `grpoxa6xbgp4`
- Credentials: `~/.oci/s3_credentials`, profile `pocketsoftware`. OCI API auth: `~/.oci/config` [DEFAULT]

🚨 **GOTCHA (banked 2026-07-14, cost us a mid-apply state-save failure):** OCI's S3-compat endpoint returns `501 NotImplemented: AWS chunked encoding not supported` on state WRITES with modern Terraform/AWS-SDK checksum defaults — even with `skip_s3_checksum = true`. Every terraform command that writes state needs:

```bash
export AWS_REQUEST_CHECKSUM_CALCULATION=when_required
export AWS_RESPONSE_CHECKSUM_VALIDATION=when_required
```

The repo justfile exports these top-level — **always use `just plan` / `just apply`, not raw terraform**. If a state save ever fails anyway: Terraform writes `errored.tfstate` to the repo root → fix env → `terraform state push errored.tfstate` → verify `terraform state list` → delete errored.tfstate. Do NOT re-apply before pushing the rescued state (forked-state risk).

## 4. Dokploy (fresh org — NOT xerox's)

- UI: `http://100.102.73.112:3000` — **TAILNET-ONLY**. Port 3000 is deliberately absent from the OCI security list and UFW. NEVER add it. Verify after any firewall change: `curl --max-time 8 http://163.176.231.29:3000` must TIME OUT.
- API: reachable **directly from the Mac over the tailnet** (no SSH hop, unlike xerox): `curl -H "x-api-key: $TOKEN" http://100.102.73.112:3000/api/<endpoint>`
- Token + admin credentials: **`peppy/secrets` mempalace drawer** (search "pocketsoftware"). Never inline.
- Known quirk: Dokploy auto-appends a random suffix to `appName` (e.g. `smoke-test-a1b2c3` → `smoke-test-a1b2c3-xzeddp`); container names follow the compose YAML, routing unaffected. `serviceName` in domain.create must match the compose service key.
- Live compose services (composeIds):

| Compose project | composeId | Notes |
|-----------------|-----------|-------|
| `smoke-test` | `LJNUSXC2aBAiB-3-SPDap` | https://test.pocketsoftware.com.br — safe to replace |
| `secrets` (Infisical) | `LdNVw0rTWKSuPIcjPQUnY` | image `infisical/infisical:v0.146.0-postgres`; tailnet-only :3005 |
| `databases` (Postgres) | `jka-xKKG232F0Jetb2w_v` | `platform-postgres:5432` on dokploy-network; zero host ports |
| `storage` (MinIO) | `YuH5TqYxOnGxY5UXnHNTC` | image `minio/minio:RELEASE.2025-09-07T16-13-09Z`; S3 public at s3.pocketsoftware.com.br |
| `observability` (obs-stack) | `iU9vjea42NJzGFZDQEJnq` | Prometheus + Grafana + Alertmanager + cAdvisor; tailnet-only |

## 5. Access Model

- Public surface: **22 (break-glass SSH, key-only + fail2ban) + 80/443 (Traefik)**. Everything else closed.
- Primary admin path: Tailscale (SSH + Dokploy UI). Public 22 stays until Tailscale proves long-term stable, then may close (operator decision).
- Hardening live: UFW (deny-in default, 22/80/443 + tailscale0), fail2ban sshd jail (5 tries/10min → 1h ban), `PasswordAuthentication no`, unattended-upgrades.
- ⚠️ Docker-published ports bypass UFW via iptables — the **OCI security list is the authoritative public firewall**; UFW is defense-in-depth.

## 6. Backups + exercised restore

- Boot-volume policy (Terraform-managed): daily incremental 06:00 UTC (7d retention) + weekly Sunday 07:00 UTC (28d).
- **Restore was exercised 2026-07-14**: backup→AVAILABLE 31s, restore→AVAILABLE 32s. Full procedure in the repo's `AGENTS.md`. Key facts: restore flag is `--boot-volume-backup-id` (NOT `--source-boot-volume-backup-id`); `is-hydrated: false` on a restored volume is normal (usable immediately). Restored drill volumes cost ~$13/mo — delete after verification (operator-gated).
- Volume snapshots are crash-consistent, not app-consistent: the day a real database lands on this server, add app-level dumps (pg_dump streamed off-host, same pattern as `incluir-prod-backup`) — file the bead then.

## 7. Provisioning (Ansible)

`ansible/setup-instance.yml` in the repo — 3 idempotent plays (base+Docker CE noble/arm64, Dokploy+Tailscale, hardening). Re-run any time: `just ansible` (mutative tier). Syntax check: `just ansible-check` (free). Tailscale join is guarded (BackendState=Running) — a re-run never needs an auth key unless the server was wiped; then mint a new single-use key (operator).

## 8. Monitoring

> All pocketsoftware observability components are **tailnet-only**. Public probes time out — verified live 2026-07-14.

### Component URLs

| Component | URL | Notes |
|-----------|-----|-------|
| Grafana | `http://100.102.73.112:3001` | pocketsoftware; admin creds in `peppy/secrets` drawer |
| Prometheus | `http://100.102.73.112:9090` | pocketsoftware; no auth |
| Alertmanager | `http://100.102.73.112:9093` | pocketsoftware; no auth |
| node_exporter | `http://100.102.73.112:9100` | pocketsoftware |
| cAdvisor | `http://100.102.73.112:8081` | pocketsoftware |
| Uptime Kuma | `http://100.88.209.119:3001` | Mac Mini watchtower; creds in `peppy/secrets` drawer |
| WAHA | `http://100.88.209.119:3002` | Mac Mini watchtower; dashboard creds in `peppy/secrets` drawer |
| Alert relay | `http://100.88.209.119:8090` | Mac Mini; `GET /` → `ok` (health probe) |

**Tailnet-only probes (both verified 2026-07-14 — use these to confirm config is correct):**
```bash
# Tailnet — must succeed (200 or 302):
curl -o /dev/null -w '%{http_code}' --max-time 8 http://100.102.73.112:3001
# Public — must time out (000):
curl -o /dev/null -w '%{http_code}' --max-time 8 http://163.176.231.29:3001
```

### Alert Channels

| Channel | When | Method |
|---------|------|--------|
| Operator DM (`553193914426@c.us`) | all alerts (warning + critical) | Alertmanager → relay `100.88.209.119:8090` → WAHA `/api/sendText` |
| Gmail SMTP (`franciscomateusvg@gmail.com`) | critical severity only | Alertmanager `critical-multi` receiver |

### Kuma Monitors

7 monitors provisioned by `mini-watchtower/kuma-monitors.py` (idempotent). To add a monitor, edit the `MONITORS` list in that file and re-run the script. Credentials (`KUMA_USER`, `KUMA_PASS`, `WAHA_API_KEY`, `CHAT_ID`) come from the **`peppy/secrets` mempalace drawer** — never hardcode them.

```bash
# Run provisioning (from pocketsoftware-terraform repo root):
python3 -m venv /tmp/kuma-venv && /tmp/kuma-venv/bin/pip install uptime-kuma-api
KUMA_URL=http://100.88.209.119:3001 KUMA_USER=<drawer> KUMA_PASS=<drawer> \
  WAHA_API_KEY=<drawer> CHAT_ID=<drawer> \
  /tmp/kuma-venv/bin/python3 mini-watchtower/kuma-monitors.py
```

## 9. Secrets (Infisical)

> **API-FIRST MANAGEMENT PRINCIPLE (operator directive — encode this everywhere):** Every platform service (Dokploy, Infisical, Kuma, future Postgres/MinIO/GlitchTip) gets a Peppy-owned API credential banked in the `peppy/secrets` drawer at bootstrap time. Operator clicks are for gates/approvals ONLY — never routine management. One-time browser-automation bootstrap is the approved pattern for services without a headless path (precedent: peppy-admin machine identity, 2026-07-14).

### URLs

| Endpoint | URL |
|----------|-----|
| UI / API (tailnet) | `http://100.102.73.112:3005` — tailnet-only, port 3005 NOT in OCI security list |
| Internal (container network) | `http://infisical-backend:8080` — reachable on `dokploy-network` |

Probe pair: `curl --max-time 8 http://100.102.73.112:3005` must return 200/302; `curl --max-time 8 http://163.176.231.29:3005` must time out (000).

### Credentials

All values (admin login, ENCRYPTION_KEY, AUTH_SECRET, DB password, machine-identity registry) live in the **`peppy/secrets` mempalace drawer** ("PocketSoftware Infisical"). **Never write values in any file or report** — always point here.

### Per-App Onboarding Ritual

1. Create a project in Infisical. Record `projectId`.
2. Create a machine identity (Universal Auth). Capture `CLIENT_ID` + `CLIENT_SECRET` → bank in drawer immediately.
3. Grant viewer: `POST /api/v2/workspace/{projectId}/identity-memberships/{identityId}` `{"role":"viewer"}`.
4. Dokploy env for the app carries **only** three vars: `INFISICAL_API_URL`, `INFISICAL_CLIENT_ID`, `INFISICAL_CLIENT_SECRET`.
5. App entrypoint: `infisical run --projectId <id> --env prod -- <cmd>` (CLI pinned `@infisical/cli@0.42.6`).

Zero app secrets touch Dokploy env fields.

### Working API Sequence (v0.146)

```bash
# Auth
POST /api/v1/auth/universal-auth/login  {clientId, clientSecret}  → Bearer accessToken

# Create project
POST /api/v2/workspace  {name, organizationId}

# Write secret
POST /api/v3/secrets/raw/<NAME>  {workspaceId, environment, secretValue}

# Grant viewer
POST /api/v2/workspace/{projectId}/identity-memberships/{identityId}  {role: "viewer"}
```

### DR Procedure (exercised 2026-07-14, total 1m41s)

Restore dump (from `mini:~/pocketsoftware-backups/infisical/`) to scratch Postgres (`--no-owner` if ownership errors) + scratch Redis + `infisical/infisical:v0.146.0-postgres` with the **same ENCRYPTION_KEY + AUTH_SECRET** from KEYS.txt → universal-auth login → read known secret back. Full recovery = dump + escrowed keys, nothing else.

Backup architecture: Mini cron 05:30 UTC daily, 14d retention, dedicated key `~/.ssh/pocketsoftware-backup`, dumps land at `mini:~/pocketsoftware-backups/infisical/`, KEYS.txt escrow beside dumps (chmod 600). Recipe: `just infisical-backup-status`.

Full DR commands and gotchas in `pocketsoftware-terraform/AGENTS.md § 8. Secrets (Infisical)`.

---

## 10. Databases (platform-postgres)

Central shared Postgres 17 for all pocketsoftware apps. Live since 2026-07-14 (BEADS `aperture-sazvl`).

### Key Facts

| Field | Value |
|-------|-------|
| Compose project | `databases` (composeId `jka-xKKG232F0Jetb2w_v`) |
| Internal DNS | `platform-postgres:5432` on `dokploy-network` |
| Published ports | **NONE** — zero by design, verified public-closed + tailnet-closed |
| Superuser credentials | `peppy/secrets` drawer — never inline, never given to apps |

### Provisioning Ritual

```bash
# From pocketsoftware-terraform repo root:
databases/provision-db.sh <app_name>
# → prints DATABASE_URL, then store it in Infisical (peppy-admin API)
# → backups pick up the new DB automatically (dynamic enumeration)
```

What the script does: `CREATE DATABASE` + `CREATE USER` + `GRANT PRIVILEGES` + **`REVOKE CONNECT FROM PUBLIC`** (isolation-critical — see gotcha xviii below) + `GRANT ALL ON SCHEMA public` (PG15+ requirement).

Full runbook: `pocketsoftware-terraform/AGENTS.md §9`.

### Connection Pattern

Apps receive `DATABASE_URL` via Infisical injection (`infisical run -- <cmd>`). Internal host: `platform-postgres:5432` on `dokploy-network`. Zero app has superuser access.

### No-Superuser Rule

No app ever receives superuser credentials. Admin access = `docker exec platform-postgres psql -U postgres` over SSH only. Superuser password: `peppy/secrets` drawer.

### Backups + Restore

- **Backup**: Mini cron 05:40 UTC daily, `pg_dump --format=custom` per DB, 14d retention, `mini:~/pocketsoftware-backups/platform-postgres/`. Dynamic enumeration — new apps auto-included. Recipe: `just pg-backup-status`.
- **Restore drill** (EXERCISED 2026-07-14, ~11s): dump→Mini→server→scratch container→`pg_restore --no-owner`→row 42 verified.
- **DR note**: run `provision-db.sh` first to recreate the app user before `pg_restore`, otherwise benign role-grant warnings appear.

Full restore commands: `pocketsoftware-terraform/AGENTS.md §9`.

### Split Triggers

Move an app to its own Postgres when:
- One app dominating CPU/IO on the shared instance
- Connection limits hit (PostgresConnectionsHigh alert sustained)
- Backup windows becoming too long
- App becoming business-critical needing full isolation

Splitting = update `DATABASE_URL` in Infisical + redeploy. Zero code change.

### PgBouncer (DEFERRED)

**Do not pre-install.** Infisical layer makes retrofit a 5-minute secret swap. Pre-installing risks wrong pool mode (transaction pooling breaks prepared statements / `LISTEN/NOTIFY`). Tripwire: `PostgresConnectionsHigh` alert. Bead: `aperture-n6ukt`.

### Alert Runbook

| Alert | Severity | Response |
|-------|----------|----------|
| `PostgresDown` | critical | All apps lose DB. Restart `platform-postgres` immediately. Drill passed 2026-07-14. |
| `PostgresConnectionsHigh` | warning | Review `pg_stat_activity`; consider PgBouncer (bead `aperture-n6ukt`). |

### Drawer Pointer

Superuser password + composeId + app DB entries: `peppy/secrets` mempalace drawer.

---

## 11. Storage (MinIO)

MinIO object storage — live since 2026-07-14, BEADS `aperture-n6ukt`. Implements the 3-2-1 backup doctrine for app file storage.

### Endpoints

| Endpoint | URL | Access |
|----------|-----|--------|
| S3 API (public) | `https://s3.pocketsoftware.com.br` | Public — LE cert via Traefik. Anonymous path: `/minio/health/live` only |
| Console | `http://100.102.73.112:9001` | Tailnet-only — OCI SL blocks publicly (verified timeout) |
| Port 9000 | — | **NEVER host-published** — internal via `dokploy-network` only |

Root MinIO credentials + replicator secret + per-app keys: **`peppy/secrets` mempalace drawer — never inline.**

### Bucket Ritual

```bash
# From pocketsoftware-terraform repo root:
minio/provision-bucket.sh <app_name>
# → creates <app>-data bucket + <app>_s3 user + bucket-scoped policy
# → prints five S3_* values
```

After the script:
1. Store all five `S3_*` values in the app's Infisical project (peppy-admin API)
2. If the bucket holds user data, add `<app>-data` to `minio/replication-list.txt`

**Public-read doctrine:** `mc anonymous set download` is NEVER a default — operator-gated decision per bucket.

Full bucket ritual + provisioning script details: `pocketsoftware-terraform/AGENTS.md §10`.

### Replication Brain (3-2-1: server → Mini → OCI)

- **Brain**: Mac Mini, `mini-watchtower/oci-replication.sh`, cron **05:55 UTC** daily
- **Nightly ballet**: 05:30 Infisical backup → 05:40 Postgres dumps → 05:55 replication to OCI
- **Sync semantics**: `rclone sync` for MinIO buckets (mirrors deletions; OCI versioning preserves history) + `rclone copy` for backup dirs (never deletes remote)
- **rclone remotes**: `[minio]` uses a custom `replicator-policy` user (built-in `readonly` lacks `s3:ListBucket`) + `[oci]` uses customer secret key #2
- **OCI bucket**: `pocketsoftware-replica` (versioned, Terraform-managed `replica-bucket.tf`)
- **Recipe**: `just replication-status`
- **3-2-1 verified 2026-07-14**: `minio/demoapp-data/drill.txt` + all dumps + `backups/infisical/KEYS.txt` confirmed in OCI replica

### Cloud Restore Drill (EXERCISED 2026-07-14 — 1m36s)

Object deleted → mc stat confirmed absence (403, not 404 — see gotcha 24) → `rclone copy oci:pocketsoftware-replica/minio/<bucket>/<obj>` → scp relay to server → `mc cp` with root alias → byte-match verified. Total: 1m36s.

OCI versioning additionally protects against bad syncs — previous object versions recoverable via `oci os object list-object-versions`.

Full restore commands: `pocketsoftware-terraform/AGENTS.md §10 — Cloud Restore Procedure`.

---

## 12. Banked Gotchas

1. **OCI S3-compat state 501** — see §3. Symptom: apply succeeds creating resources, then "Failed to persist state". Never lose the errored.tfstate.
2. **OCI customer-secret-keys have a ~3-4 min propagation delay** before the S3-compat API accepts them (fresh keys fail auth briefly).
3. **RTK hook truncates long terraform output** mid-stream — for full plans use `rtk proxy terraform plan` (or `just plan`).
4. **Dokploy appName auto-suffix** — see §4.
5. **just brace escaping**: `{{{{.Names}}` in justfile → `{{.Names}}` in shell (docker Go-templates).
6. **WAHA `:latest` is amd64-only** (banked 2026-07-14): `devlikeapro/waha:latest` is an amd64 image. On arm64 (Mac Mini M-series) use `devlikeapro/waha:arm` tag — the `arm` tag tracks the latest arm64-compatible build. Symptom: container exits immediately or crashes with architecture mismatch.
7. **WAHA engine must be NOWEB on arm64** (banked 2026-07-14): the WEBJS engine uses Puppeteer/Chromium which crashes on arm64 (no matching Chromium build). Set `WHATSAPP_DEFAULT_ENGINE=NOWEB` in the WAHA container env. NOWEB is a lightweight no-browser engine that works reliably on arm64.
8. **rsync `--delete` removes `.env`** (banked 2026-07-14): `rsync -az --delete mini-watchtower/ mini:pocketsoftware-watchtower/` will delete any file on the remote that is not in the local source — including the `.env` file (which is gitignored and lives only on the Mini). Always add `--exclude .env` to the rsync command. Without it, the stack loses its secrets and all containers exit.
9. **WAHA dashboard creds auto-generate in container logs** (banked 2026-07-14): on first start, WAHA generates a random admin password and prints it once to stdout. Run `docker logs watchtower-waha 2>&1 | grep -i password` immediately after first start to capture it. Bank in `peppy/secrets` drawer — it is NOT recoverable later without resetting the container data.
10. **WhatsApp canonical chat-id drops the Brazilian ninth digit** (banked 2026-07-14): Brazilian mobile numbers added a 9th digit around 2012, but WhatsApp's internal representation still uses the 8-digit legacy format. Example: number `(31) 99391-4426` → WAHA chat-id `553193914426@c.us` (not `5531993914426@c.us`). Test with a relay `POST /alert` and confirm delivery before banking the id.
11. **Push before Dokploy deploy** (banked 2026-07-14): Dokploy's compose deploy clones directly from GitHub (`origin/main`) at deploy time. If local commits haven't been pushed yet, the deploy fails with `Compose file not found`. Always `git push origin main` before triggering any Dokploy deploy — especially when multiple tasks write files before the first push.
12. **Kuma 1.x has no native WAHA notification provider** (banked 2026-07-14): the `type="waha"` notification type exists only in Kuma 2.x. On Kuma 1.23.x (used here), use `NotificationType.WEBHOOK` with `webhookContentType="custom"` and a LiquidJS body template that emits WAHA's `sendText` JSON payload. The `uptime-kuma-api` v1.x library also has no WAHA entry in `NotificationType` — passing an unknown type raises `TypeError`.

13. **`infisical-migrate` requires the FULL env** (banked 2026-07-14): the migrate command validates ALL env vars including `ENCRYPTION_KEY`. The migrate service must have the complete env set (same as infisical-backend), not just `DB_CONNECTION_URI`. Symptom: migrate exits with env validation error on first deploy.
14. **Failed migrate leaves `is_locked=1`** (banked 2026-07-14): a crashed migrate run sets `infisical_migrations_lock.is_locked=1` in Postgres. Clear it via `UPDATE infisical_migrations_lock SET is_locked=0` before redeploying — otherwise next migrate exits immediately without running and the backend never starts.
15. **CLI/server version skew: pin `@infisical/cli@0.42.6`** (banked 2026-07-14): latest `@infisical/cli` speaks `/api/v4/secrets` → 404 on server v0.146. Always pin `@infisical/cli@0.42.6` in app images and throwaway containers paired with this server version.
16. **Tailscale SSH intercepts tailnet-ip:22** (banked 2026-07-14): automation from the Mini targeting `100.102.73.112:22` triggers Tailscale's interactive re-auth flow (not a standard SSH session). Use the **public IP `163.176.231.29:22`** + dedicated key `~/.ssh/pocketsoftware-backup` for any machine-to-machine automation. Stage-2 hardening (restricted command) is a deferred bead.
17. **Image tags can be stale — always `docker manifest inspect` before pinning** (banked 2026-07-14): `v0.151.0-postgres` never existed; the plan pin was a guess. Run `docker manifest inspect infisical/infisical:<tag>` to verify a tag exists and has an arm64 layer before committing it to the compose file.
18. **GitHub webhooks cannot reach tailnet-only Dokploy — `autoDeploy` is structurally impossible** (2026-07-14): Dokploy port 3000 is not publicly reachable; GitHub cannot deliver webhooks. `autoDeploy=true` never fires. ALL deploys on this server are API-triggered (`POST /api/compose.deploy`). Ignore any claim that autoDeploy works here.
19. **`REVOKE CONNECT FROM PUBLIC` is isolation-critical in the Postgres ritual** (2026-07-14): Postgres grants `CONNECT` to `PUBLIC` on every new database by default. Without the revoke step, any DB user can connect to any other app's DB. The `databases/provision-db.sh` script includes this step — never skip it.
20. **`pg_restore` cross-env role warnings are benign — but ritual-before-restore is the correct DR procedure** (2026-07-14): when restoring to a fresh environment, `pg_restore` emits `ERROR: role "<app>_user" does not exist`. Symptom: 1 error ignored. Fix: run `provision-db.sh` first to recreate the user, then `pg_restore --no-owner`. The warning is benign (data restores correctly), but the ritual is the right procedure.
21. **Dispatch secrets must be read from source-of-truth at dispatch time, never from controller memory** (2026-07-14): a worker was dispatched with a hallucinated secret from controller memory instead of `terraform output`. The worker self-healed by pulling the real value. Always fetch sensitive outputs fresh (e.g. `terraform output -raw replication-secret`) at the moment of dispatch — never rely on a value you think you remember.
22. **MinIO built-in `readonly` policy lacks `s3:ListBucket` — rclone sync needs a custom policy** (2026-07-14): attaching `readonly` to the replicator user causes `rclone sync` to fail (cannot list bucket contents). Create a custom `replicator-policy` that includes `s3:ListBucket` alongside `s3:GetObject`. Symptom: rclone exits with `AccessDenied` on bucket listing even though credentials are correct.
23. **`minio/mc` image default entrypoint is `mc` — use `--entrypoint sh` for pipelines** (2026-07-14): `docker run minio/mc -c "..."` fails because the default entrypoint is `mc`. Always use `docker run --entrypoint sh minio/mc:<tag> -c "<commands>"` for any multi-step shell pipeline.
24. **MinIO returns 403, not 404, for missing objects on private buckets** (2026-07-14): a scoped key against a non-existent object returns `403 AccessDenied`, not `404`. The correct absence check is `mc stat` — output "Object does not exist" means it's truly gone; 403 from a scoped key is ambiguous (could be missing or permission-denied). Use root alias with `mc stat` for unambiguous absence verification.
25. **`infisical` CLI 0.42.6 does not auto-consume `MACHINE_IDENTITY_*` env vars; alpine needs `ca-certificates`** (2026-07-14): fetch the token via the universal-auth API and pass it with `--token` explicitly. Alpine images also need `apk add ca-certificates` before any HTTPS call to a Let's Encrypt-signed endpoint, including `s3.pocketsoftware.com.br`.
26. **`S3_FORCE_PATH_STYLE` is not an aws-cli env var** (2026-07-14): the aws CLI does not read `S3_FORCE_PATH_STYLE` from the environment. Pass `--endpoint-url https://s3.pocketsoftware.com.br` directly on every CLI invocation. SDKs use `forcePathStyle: true` in client config. The env var is only a documentation convention for passing the value to app processes.
27. **Dokploy `compose.deploy` does not recreate unchanged containers** (2026-07-14): if only a mounted config file changed (e.g. `prometheus.yml`) but the compose definition didn't, the redeploy leaves the container running with the old config. For Prometheus config changes: `docker restart obs-prometheus` after deploy, or add `--web.enable-lifecycle` to the Prometheus command and `POST http://obs-prometheus:9090/-/reload`.
<!-- Add new gotchas above this line, numbered, with date + symptom + fix. -->
