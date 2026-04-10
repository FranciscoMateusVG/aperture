---
name: aperture-deploy-workflow
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
| **Build** | GLaDOS (or spiderling) | Scaffolds the app, writes code, creates Dockerfile + docker-compose.yml. |
| **Push** | Builder | Pushes to GitHub on `main` branch. Verifies branch exists with `git ls-remote`. |
| **Handoff** | Builder → Peppy | Sends structured deploy spec (see format below). |
| **Deploy** | Peppy | Creates Dokploy compose service, configures domain, triggers deploy via API. |
| **Verify** | Peppy | Confirms HTTPS is live, cert is valid, app responds. |
| **Report** | Peppy → GLaDOS → Operator | Reports live URL, compose ID, status. |

---

## 2. Role Responsibilities

**GLaDOS (Orchestrator)**
- Reviews and approves all plans before execution
- Decides execution strategy: code it herself, spawn spiderlings, or delegate
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
