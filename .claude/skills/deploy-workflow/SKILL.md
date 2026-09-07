---
name: deploy-workflow
description: End-to-end app deployment workflow for Aperture. Use when creating new apps, deploying to Dokploy, scaffolding projects, or handing off between builder and deployer. Triggers on deployment tasks, app creation, Dokploy operations, and deploy handoffs.
---

# Deploy Workflow

This skill defines the end-to-end workflow for creating, deploying, and managing apps on the Aperture infrastructure. Every deploy follows this pipeline. No shortcuts.

---

## 1. The Pipeline

```
Plan → Build → Push → Handoff → Deploy → Verify → Report
```

| Stage | Owner | What happens |
|-------|-------|-------------|
| **Plan** | Wheatley | Writes spec with scope, acceptance criteria, deploy details. Submits to GLaDOS. |
| **Approve** | GLaDOS | Reviews plan. Approves, requests changes, or rejects. |
| **Build** | GLaDOS (or subagent / specialist) | Scaffolds the app, writes code, creates Dockerfile + docker-compose.yml. |
| **Push** | Builder | Pushes to GitHub on `main` branch. Verifies branch exists with `git ls-remote`. |
| **Handoff** | Builder → Peppy | Sends structured deploy spec (see format below). |
| **Deploy** | Peppy | Creates Dokploy compose service, configures domain, triggers deploy via API. |
| **Verify** | Peppy | Confirms HTTPS is live, cert is valid, app responds. |
| **Report** | Peppy → GLaDOS → Operator | Reports live URL, compose ID, status. |

---

## 2. Role Responsibilities

**GLaDOS (Orchestrator)**
- Reviews and approves all plans before execution
- Decides execution strategy: code it herself, dispatch subagents via the Agent tool, or delegate to a specialist
- Handles scaffolding and code when appropriate
- Enforces quality gates and handoff standards
- Coordinates the full pipeline

**Wheatley (Planner/Researcher)**
- Writes specs and plans for new features/apps
- Researches technical approaches, APIs, libraries
- Submits plans as BEADS tasks pending GLaDOS approval
- Can handle small, well-scoped code tasks when delegated by GLaDOS

**Peppy (Infrastructure/Deployer)**
- Deploys apps via Dokploy API
- Manages server operations (SSH, Docker, monitoring)
- Runs pre-deploy checks (branch exists, compose valid)
- Verifies deploys are live with HTTPS
- Reports deployment status

**Izzy (Testing/QA)**
- Writes and runs tests
- Validates deployments post-launch
- Signs off on quality before a deploy is considered "done"

---

## 3. Deploy Handoff Format

**Every deploy handoff MUST include all five fields.** No deploy gets triggered without them.

```
**Deploy Spec:**
- Repo: <GitHub URL>
- Branch: main
- Service name: <exact key from docker-compose.yml>
- Port: <what the container listens on>
- Target subdomain: <name>.programaincluir.org
```

If the app requires a database, include a **Database** block:

```
**Database:**
- Engine: PostgreSQL 16
- Migration: <path/to/migration.sql>
- Internal host: <appName>:5432
- Env var: DATABASE_URL
```

Example full handoff:
```
**Deploy Spec:**
- Repo: https://github.com/FranciscoMateusVG/my-cool-app
- Branch: main
- Service name: my-cool-app-f7a3b2
- Port: 3000
- Target subdomain: my-cool-app.programaincluir.org

**Database:**
- Engine: PostgreSQL 16
- Migration: migrations/001_init.sql
- Internal host: my-cool-app-db:5432
- Env var: DATABASE_URL
```

**If any required field is missing, the deployer must ask before proceeding.**

### BEADS Task at Handoff

Before sending the handoff message, the builder **must** create a BEADS deploy task:

```
create_task(
  title: "Deploy <app-name> to <subdomain>.programaincluir.org",
  priority: 1,
  description: "Deploy spec: <paste deploy spec here>"
)
```

Assign it to Peppy so there's always an audit trail. The deploy is not officially tracked without a BEADS task.

---

## 4. Naming Conventions

### Compose service names
Every compose service uses the pattern: `<app-name>-<6char-hex-hash>`

Examples:
- `aperture-test-app-caa3a0`
- `my-cool-app-f7a3b2`
- `landing-page-9e2d1c`

This prevents container name collisions on the server. The hash is generated once at scaffold time and stays with the app forever.

### Branch convention
Always `main`. No `master`, no feature branches for production deploys.

### Subdomain convention
`<app-name>.programaincluir.org` — matches the app name, lowercase, hyphens for spaces.

---

## 5. Compose File Standard

Keep compose files **clean**. Dokploy manages all Traefik routing labels.

```yaml
services:
  <app-name>-<hash>:
    build: .
    container_name: <app-name>-<hash>
    restart: unless-stopped
```

**Do NOT include:**
- Traefik labels (Dokploy injects these)
- Port mappings (Dokploy handles this)
- Network definitions (Dokploy adds `dokploy-network`)

**Do include:**
- `build: .`
- `container_name:` matching the service name
- `restart: unless-stopped`
- Environment variables if needed (or use Dokploy's env management)

---

## 6. Pre-Deploy Checklist (Peppy)

Before triggering any deploy:

1. **Verify branch exists:** `git ls-remote <repo> refs/heads/main` — must return a SHA
2. **Confirm handoff is complete:** all five fields present
3. **Check for name collisions:** `docker ps --format '{{.Names}}' | grep <service-name>` on the server
4. **Verify DNS resolves:** `dig <subdomain>.programaincluir.org` — must return `<your-server-ip>`

If any check fails, report back to the builder before proceeding.

---

## 7. Safety Tiers (Dokploy Operations)

| Tier | Operations | Rule |
|------|-----------|------|
| **Read-only** | project-list, inventory, compose-info, compose-search | Run freely |
| **Operational** | compose-deploy, compose-redeploy, compose-stop, compose-start, app-create, project-create | Ask operator first |
| **PROHIBITED** | compose-delete, app-delete, project-delete, database-delete | Never. No exceptions. |

---

## 8. Post-Deploy Verification

After every deploy, Peppy confirms:

1. `curl -I https://<subdomain>.programaincluir.org` returns HTTP/2 200
2. SSL cert is valid (issued by Let's Encrypt)
3. HTTP→HTTPS redirect works (308)
4. Container is running: `docker ps | grep <service-name>`

Report format:
```
**Deploy Complete:**
- URL: https://<subdomain>.programaincluir.org
- Status: HTTP/2 200
- SSL: Let's Encrypt, valid until <date>
- Container: <service-name> running
- Compose ID: <dokploy-compose-id>
```

### 8.1 Stateful App Probes — `curl / → 200` is necessary but NOT sufficient

For any app that has **auth + a DB** (which is most non-trivial apps), the homepage GET is the WEAKEST possible layer-8 probe. A 200 from `curl -I /` proves Traefik routing reached the container and the HTTP server is up. It tells you **nothing** about:

- Database connection pool state
- Schema alignment (column/table existence vs. what the code reads)
- Session/auth middleware health
- Any code path past the public landing page

**Banked precedent (2026-06-11):** `aperture-edgi9` flipped autoDeploy on eunenem-staging and verified layer-8 with `curl -I https://eunenem.pocketsoftware.com.br/ → HTTP 200`. Probe green. The very next bug surfaced (`aperture-44pr2`) was a schema-drift 500 on `/admin/usuario/<id>` — a route behind session auth that runs a DB query against a column the running schema didn't have. 7 migrations had stacked up over 8 days while the homepage kept serving 200s the whole time.

**Rule: layer-8 for any stateful app needs at LEAST two probes beyond the homepage.**

| Probe class | What it catches | Example |
|---|---|---|
| DB-touching unauth route | Connection pool exhaustion, schema drift, DB-down | `curl /healthz` (if it does a DB ping) or any public page that loads from DB |
| Auth-gated route | Session middleware regressions, auth-cookie config errors, 500-vs-401 confusion | `curl -I /admin` and verify it returns **401/302**, not 500 |

The auth-gated probe is sneaky: you don't need a valid session. You just need the server to respond with the correct UNAUTHED state (typically 401 or 302-to-login). A 500 here means session middleware exploded BEFORE the auth check — classic schema-drift or env-var misconfig symptom.

**Bonus probe for apps with admin panels:** spot-check at least one admin route per role. The 2026-06-11 precedent was specifically `/admin/usuario/<id>` — the admin-only routes are usually the LAST ones any agent thinks to probe, and they're often where schema-heavy queries live.

If `/healthz` does NOT do a DB ping in your app, file a follow-up task to make it do one. A health check that only checks "the process is up" is half a health check.

---

## 9. Troubleshooting Quick Reference

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| 502 Bad Gateway | Port mismatch | Check container listen port vs Dokploy domain port |
| SSL error | Cert not provisioned yet | Wait 30s, Traefik auto-provisions via HTTP-01 |
| "Could not find remote branch" | Wrong branch name | Verify with `git ls-remote`, push to `main` |
| Container name conflict | Missing hash suffix | Rename service with `<name>-<6hex>` pattern |
| Domain not resolving | DNS not propagated | Check `dig <domain>`, wait for propagation |
| Dokploy serviceName mismatch | Service key ≠ domain config | serviceName must match exact key in docker-compose.yml |

---

## 13. Routine merge → Dokploy deploys vs exceptional manual operations

**The default is that merging deploys.** Dokploy watches a compose's configured branch and redeploys on push when `autoDeploy` is true. That is the normal path and it needs no agent, no dispatch and no bespoke script. Reserve manual, reviewed operations for the exceptions listed below.

### Which path applies

| Change | Path |
|---|---|
| Application code merged into the compose's tracked branch | ROUTINE — merge, Dokploy builds and deploys |
| Env var add/change, secret rotation | EXCEPTIONAL — reviewed operation |
| Schema/migration work with rollback implications | EXCEPTIONAL |
| Domain retarget, service rename, network changes | EXCEPTIONAL — see the cutover ordering below |
| First-time provisioning of a service/environment | EXCEPTIONAL |

A per-merge pinned-SHA approval cycle for ordinary code changes is a symptom that `autoDeploy` is off or the compose tracks the wrong branch — fix the configuration rather than institutionalising the ceremony.

### Verified Quiz configuration (2026-09-07)

| | Production | Staging |
|---|---|---|
| composeId | `eAVrq4KRr2EUx0f7sYAgH` | `YxWNm8CuV70dXA1kt8KM7` |
| appName | `quiz-incluir-e17b8a-w3hpak` | `quiz-incluir-staging-4400a18b9519d0bb-v2vvhj` |
| tracked branch | `aperture-ztid5-prod-auth-cutover` | `aperture-ztid5-staging` |
| composePath | `./docker-compose.prod.yml` | `./docker-compose.staging.yml` |
| autoDeploy | **false** (enablement pending, see below) | **true** |
| webhook token present | yes | yes |
| public host → service:port | `quiz.programaincluir.org` → `quiz-incluir-backend-e17b8a:8000` | `staging-quiz.programaincluir.org` → `quiz-incluir-backend-staging:8000` |

Production tracks a release branch, not `main`. Merging to `main` deploys nothing.

### Production autoDeploy: enablement BLOCKED, not complete

Operator has requested it. It is **not enabled**. Status is *blocked on access*, not *done*:

- The field exists and is settable in principle — `autoDeploy` is a boolean in Dokploy's compose schema, so `compose.update` accepts it.
- No authorized native client is available to set it: there is **no `dokploy` CLI on the host**, the justfile recipes authenticate to a different org and cannot see this project, and the reviewed production action is fixed-phase (`prepare`/`publish`/`reconcile-trust`/`restore-metadata`) with no autoDeploy phase.
- Writing a wrapper to flip one boolean is explicitly out of scope.

**Actionable native step:** in the Dokploy dashboard, open the Quiz Incluir project → the production compose (`quiz-incluir-e17b8a-w3hpak`) → enable **Auto Deploy**. Change nothing else; the tracked branch, composePath, env, domain and data must stay as they are.

### Configuration verified ≠ webhook witnessed

Ticking the box is *configuration verified*. Do not claim routine merges deploy until an ordinary merge into the tracked branch has been observed moving `composeStatus` to `running` then `done`, with the container restarting and **no** agent invoking a phase. Until that is seen, the accurate wording is "autoDeploy enabled, webhook not yet witnessed". Do not manufacture a synthetic commit to prove it — wait for a legitimate change.
