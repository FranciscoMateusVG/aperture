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

## 8. Banked Gotchas

1. **OCI S3-compat state 501** — see §3. Symptom: apply succeeds creating resources, then "Failed to persist state". Never lose the errored.tfstate.
2. **OCI customer-secret-keys have a ~3-4 min propagation delay** before the S3-compat API accepts them (fresh keys fail auth briefly).
3. **RTK hook truncates long terraform output** mid-stream — for full plans use `rtk proxy terraform plan` (or `just plan`).
4. **Dokploy appName auto-suffix** — see §4.
5. **just brace escaping**: `{{{{.Names}}` in justfile → `{{.Names}}` in shell (docker Go-templates).

<!-- Add new gotchas above this line, numbered, with date + symptom + fix. -->
