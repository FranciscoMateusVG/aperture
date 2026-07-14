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
- Live services: `smoke-test` project, composeId `LJNUSXC2aBAiB-3-SPDap` → https://test.pocketsoftware.com.br (whoami smoke test; safe to replace when a real app needs the subdomain — operator call).

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

## 9. Banked Gotchas

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

<!-- Add new gotchas above this line, numbered, with date + symptom + fix. -->
