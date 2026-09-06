---
name: aperture-dokploy-api
description: Dokploy API reference for Aperture infrastructure operations. Use when making Dokploy API calls — creating databases, compose services, domains, or deploying. Triggers on Dokploy operations, API calls, database provisioning, and compose management.
---

# Dokploy API Reference

Quick reference for Dokploy REST API operations on the Aperture server. All calls go through SSH to `localhost:3000` on the server.

---

## 1. Authentication

There are **two separate Dokploy organizations** on the same server, each with its own API token:

### Xerox Org (BH Escape, Aperture Test, CROSS, FITT)
```bash
TOKEN=$(python3 -c "import json; print(json.load(open('/home/ubuntu/.config/@dokploy/cli/config.json'))['token'])")
```

### Incluir Org (Main App, Infra, Waha) — PROD CUSTOMER-FACING
```bash
# Source of truth: peppy/secrets drawer in mempalace.
# The token rotates; never inline it here.
# Look it up via: mcp__mempalace__mempalace_search query="dokploy api tokens"
TOKEN="<from peppy/secrets drawer>"
```

(The token `lZMOoQgl...` previously documented inline here is rotated/dead — do NOT use.)

Pass the appropriate token as: `-H "x-api-key: $TOKEN"`

**Use the correct token for the org you're operating on.** The wrong token will return an empty project list.

---

## 2. Common Endpoints

### Projects

| Endpoint | Method | Description |
|----------|--------|-------------|
| `project.all` | GET | List all projects with environments, services, databases |

### Compose Services

| Endpoint | Method | Description |
|----------|--------|-------------|
| `compose.one?composeId=ID` | GET | Get details for one compose service |
| `compose.create` | POST | Create a new compose service |
| `compose.update` | POST | Update compose service fields |
| `compose.deploy` | POST | Trigger a deploy |
| `compose.redeploy` | POST | Redeploy an existing service |
| `compose.stop` | POST | Stop a compose service |
| `compose.start` | POST | Start a stopped compose service |

### PostgreSQL Databases

| Endpoint | Method | Description |
|----------|--------|-------------|
| `postgres.create` | POST | Create a new PostgreSQL database |
| `postgres.deploy` | POST | Deploy/start the database container |

### Domains

| Endpoint | Method | Description |
|----------|--------|-------------|
| `domain.create` | POST | Create a domain routing rule |

---

## 3. Compose Service — Create + Configure

**Known quirk:** `compose.create` does NOT persist GitHub source fields (`repository`, `owner`, `branch`, `githubId`). You MUST call `compose.update` immediately after to set them.

### Step 1: Create

```bash
curl -s -X POST -H "x-api-key: $TOKEN" -H "Content-Type: application/json" \
  -d '{
    "name": "<display-name>",
    "appName": "<service-name>",
    "environmentId": "<env-id>",
    "composeType": "docker-compose",
    "composePath": "./docker-compose.yml",
    "sourceType": "github"
  }' \
  http://localhost:3000/api/compose.create
```

**Required fields:** `name`, `appName`, `environmentId`, `composeType`, `composePath`, `sourceType`

**Note:** Dokploy may append a random suffix to `appName` (e.g., `pub-quiz-8e9215` becomes `pub-quiz-8e9215-rgfreb`). The compose service name in `docker-compose.yml` must match the ORIGINAL name without Dokploy's suffix.

### Step 2: Update with GitHub source

```bash
curl -s -X POST -H "x-api-key: $TOKEN" -H "Content-Type: application/json" \
  -d '{
    "composeId": "<compose-id>",
    "repository": "<repo-name>",
    "owner": "<github-owner>",
    "branch": "main",
    "githubId": "TOmazYpTr8Wz21abongPE",
    "sourceType": "github"
  }' \
  http://localhost:3000/api/compose.update
```

**GitHub connection ID:** `TOmazYpTr8Wz21abongPE` (FranciscoMateusVG's GitHub connection — same for all repos under this account)

### Step 3: Set environment variables

```bash
curl -s -X POST -H "x-api-key: $TOKEN" -H "Content-Type: application/json" \
  -d '{
    "composeId": "<compose-id>",
    "env": "KEY1=value1\nKEY2=value2"
  }' \
  http://localhost:3000/api/compose.update
```

Env vars are newline-separated `KEY=VALUE` pairs in a single string.

---

## 4. PostgreSQL Database — Create + Deploy

### Step 1: Create

```bash
curl -s -X POST -H "x-api-key: $TOKEN" -H "Content-Type: application/json" \
  -d '{
    "name": "<display-name>",
    "appName": "<container-name>",
    "databasePassword": "<password>",
    "dockerImage": "postgres:16-alpine",
    "databaseName": "<db-name>",
    "databaseUser": "<db-user>",
    "environmentId": "<env-id>"
  }' \
  http://localhost:3000/api/postgres.create
```

**Required fields:** `name`, `appName`, `databasePassword`, `dockerImage`, `databaseName`, `databaseUser`, `environmentId`

**Note:** Dokploy appends a random suffix to `appName` here too. The returned `appName` is the actual container/service name on the Docker network.

### Step 2: Deploy

The database starts in `idle` state. You must deploy it:

```bash
curl -s -X POST -H "x-api-key: $TOKEN" -H "Content-Type: application/json" \
  -d '{"postgresId": "<postgres-id>"}' \
  http://localhost:3000/api/postgres.deploy
```

### Internal Connection String

Once deployed, the database is reachable from other containers on `dokploy-network` via:

```
postgres://<user>:<password>@<actual-appName>:5432/<dbname>
```

The `<actual-appName>` is the one returned by the API (with Dokploy's suffix), NOT the one you requested. Always read it from the create response.

---

## 5. Domain Configuration

```bash
curl -s -X POST -H "x-api-key: $TOKEN" -H "Content-Type: application/json" \
  -d '{
    "composeId": "<compose-id>",
    "host": "<subdomain>.programaincluir.org",
    "https": true,
    "port": <container-port>,
    "serviceName": "<service-key-from-docker-compose>",
    "certificateType": "letsencrypt",
    "path": "/",
    "domainType": "compose"
  }' \
  http://localhost:3000/api/domain.create
```

**Required fields:** `composeId`, `host`, `https`, `port`, `serviceName`, `certificateType`, `path`, `domainType`

**Critical:** `serviceName` must match the exact service key in `docker-compose.yml`, NOT the Dokploy `appName`.

---

## 6. Deploy

```bash
curl -s -X POST -H "x-api-key: $TOKEN" -H "Content-Type: application/json" \
  -d '{"composeId": "<compose-id>"}' \
  http://localhost:3000/api/compose.deploy
```

Returns `{"success": true, "message": "Deployment queued"}` on success.

---

## 7. Running Migrations

To run a migration after deploy, pipe from the app container (which has the migration files) into the DB container:

```bash
docker exec <app-container> cat /app/migrations/<file>.sql | \
  docker exec -i <db-container> psql -U <user> -d <dbname>
```

The app container name matches the service key in `docker-compose.yml`. The DB container name is the full Dokploy `appName` with a swarm task suffix (e.g., `pub-quiz-db-8e9215-izni4g.1.xxxxx`). Use `docker ps | grep <db-appname>` to find the exact name.

---

## 8. Known Environment IDs

### Xerox Org
| Project | Environment | ID |
|---------|-------------|-----|
| Aperture Test | production | `-gnwHYB_Sk1iPP4luBHzS` |

### Incluir Org
| Project | Environment | ID |
|---------|-------------|-----|
| Prod - Main App | production | `env_prod_nk6Ypd57ZscfiaHnQRzES_1757794241.771092` |
| Infra | production | `env_prod_OFpzQ5wgFC2C2VUscYRNV_1757794241.771092` |
| waha | production | `OyQcQ0Yk9RCnfLgkTpnhf` |

---

## 9. Known Compose IDs

### Xerox Org
| Service | Compose ID |
|---------|-----------|
| Ask Francisco | `dQXVgxC6pchh8rgOdL1dG` |
| Lucas - CROSS | `7AbeYbtJtkB3OSFumET-V` |
| Wanderson - FITT | `4QJKHyMOplCqos2KhXLNd` |
| Aperture Test App | `HLypwwLCFTj3RE6J4Zbj0` |
| Pub Quiz Scoreboard | `Lr-Pv8mxeYVD37argTlEJ` |
| Secretaria Test | `zg6mgJNJlOaYggUXWy95m` |

### Incluir Org

**⚠️ Source of truth is `peppy/secrets` drawer in mempalace.** This table is a snapshot — verify before deploying. composeIds can drift in Dokploy when projects are renamed/recreated.

| Status | Service | Compose ID | appName | Branch |
|--------|---------|-----------|---------|--------|
| 🟢 ACTIVE | **Main Apps (Prod) — hono+frontend** | `_A6rI-GEm9oF8ysIojm0O` | `compose-override-solid-state-port-349ude` | `main` |
| 🟢 ACTIVE | Observability (Loki + Tempo + Grafana) | `bPiJP-GUPhNbIsOEN_HmW` | `incluir-observability-9nbdjh` | `main` |
| 🟢 ACTIVE | Minio | `biqK8MbgAXtrJH24k5zTg` | `infra-minio-6b6568` | — |
| 🟢 ACTIVE | Unleash | `27vJsrYScdmCcKf1qVh6Y` | `infra-unleash-xthfr8` | — |
| 🟢 ACTIVE | Waha | `uIBU4__1Jw3RGp6WSzz6y` | `waha-app-8fj6ue` | `master` |
| 🛑 STOPPED 2026-05-07 | Legacy NestJS Main App | `4sHHtg1XwERiDc6o2labm` | `prod-main-app-main-apps-wfjeox` | `master` |

### Pre-deploy verification (mandatory)

Before any `compose.deploy` / `compose.stop` / `compose.redeploy`, verify the composeId still maps to the appName you expect:

```bash
ssh xerox 'docker exec dokploy-postgres.1.zos6qj3u1fm7t10d72r5yzpc0 psql -U dokploy -d dokploy -c \
"SELECT \"composeId\", name, \"appName\", branch, \"composeStatus\" FROM compose WHERE \"composeId\" = '\''<COMPOSE_ID>'\'';"'
```

If the row's `appName` doesn't match the table above, **STOP**. The composeId has drifted. Read the `peppy/secrets` drawer for the correct active mapping. Filing aperture-9oxq follow-up tracks automating this guard into the justfile recipes.

---

## 10. Safety Reminder

| Tier | Operations | Rule |
|------|-----------|------|
| **Read-only** | `.one`, `.all`, project-list, compose-info | Run freely |
| **Operational** | `.create`, `.deploy`, `.update`, `.stop`, `.start` | Operator approval required |
| **PROHIBITED** | `.delete`, `.remove` | Never. No exceptions. |

---

## 11. `compose.update` env field — JSON literal `\n` escape footgun

**TL;DR**: The `env` field on `compose.update` is a JSON string requiring literal `\n` escapes between vars. Bash `$(cat file)` → `jq --arg` → `curl` chains can silently collapse those newlines into a single concatenated mega-string. The `compose.one` round-trip returns the mangled value with `success:true`; the breakage doesn't surface until the next deploy when the env block fails to parse as KEY=VALUE pairs.

**Banked precedent** (2026-05-25, aperture-h7sq Peppy compose-env-set):

```python
# After a "successful" compose.update:
repr(env[:200])
# → 'BETTER_AUTH_SECRET=9bc4ab...ecBETTER_AUTH_URL=https://...COOKIE_DOMAIN=...'
env.count("\n")  # → 0
```

Every var concatenated into the previous one. If the next deploy had fired before recovery, every env var would have been mangled and the stack would have failed to come up. Detection only came from a paranoid `repr` + newline-count check after the round-trip.

### The shape that bites

```bash
# DON'T — bash $(cat) can survive the trip, but jq --arg + curl --data
# combine to silently lose internal newlines.
PAYLOAD=$(jq -n --arg env "$(cat env.txt)" '{composeId:"...", env:$env}')
echo "$PAYLOAD" > payload.json && scp payload.json xerox:...
```

### The shape that works

Build the JSON in Python directly, never let bash command-substitution touch the env content. Self-verify newline counts before AND after the POST.

```python
import json
env_text = open("env.txt").read().rstrip("\n")
# Pre-flight: env must have many newlines
assert env_text.count("\n") > 20, "env_text looks mangled"

payload = {"composeId": COMPOSE_ID, "env": env_text}
payload_json = json.dumps(payload, ensure_ascii=False)
# Pre-flight: JSON-encoded form must contain escape sequences
assert payload_json.count("\\n") > 20, "JSON not escaping newlines"
open("payload.json", "w").write(payload_json)
```

Then scp + curl `--data @payload.json`. Then read it back:

```bash
ssh xerox "curl -s -H 'x-api-key: $TOKEN' 'http://localhost:3000/api/compose.one?composeId=...'" | \
  python3 -c "import json,sys; print('newlines:', json.load(sys.stdin)['env'].count(chr(10)))"
```

Anything less than the line count you sent = corruption. **Do NOT redeploy until fixed.**

### Recovery procedure if you discover post-POST mangling

The mangled env is in Dokploy's stored compose-env block but NOT yet in any running container (containers carry the env baked at the LAST successful deploy). You have a window before the next auto-deploy fires:

1. Rebuild the env-text file with newlines intact
2. Re-POST `compose.update` with the corrected payload (Python builder above)
3. Re-read via `compose.one` and verify newline count
4. THEN trigger redeploy

Recovery in same session: precedent at aperture-h7sq compose-env-set 2026-05-25.

---

## 12. Dokploy compose env ≠ container env — the layer-skip trap

**TL;DR**: Setting a var in Dokploy's compose env block is **necessary but not sufficient**. The `docker-compose.yml` service block must ALSO have an `environment:` entry that interpolates the var (`${MY_VAR:-}`) for the var to cross into the container's `process.env`. Without the YAML wire-through, Dokploy stores the value at compose-project level but it never reaches the running service.

**Banked precedent** (2026-05-25, aperture-h7sq + aperture-xdn9 hot-fix PR #409):

Rex's PR #404 added `process.env.VOLUNTEER_CALENDAR_LINK` read in `apps/hono-app/src/http/server.ts`. Peppy set the var in Dokploy compose env + triggered redeploy. Layer-5 probe (`compose.one`) confirmed the var stored. But:

```bash
docker exec compose-override-solid-state-port-349ude-hono-app-1 env | grep VOLUNTEER
# → returned nothing
```

Why: `docker-compose.new-app.yml` had no entry passing the var into the hono-app service's `environment:` block. Hot-fix PR #409 added the missing line:

```yaml
hono-app:
  environment:
    VOLUNTEER_CALENDAR_LINK: ${VOLUNTEER_CALENDAR_LINK:-}
```

Post-#409 deploy: `docker exec hono-app env | grep VOLUNTEER` returned the value. Chain closed.

### The canonical verify chain — every layer, every artifact

| Layer | Probe | Expected |
|-------|-------|----------|
| 5 — Dokploy stored | `compose.one` response's `env` field grep | var present with value |
| 6a — Compose YAML wire | grep `${VAR` in `docker-compose.<name>.yml` for the consuming service | one match in the right service's `environment:` block |
| 6b — Container baked | `docker exec <service-container> env \| grep VAR` | var present with value |
| 7 — Code reads it | `grep "process.env.VAR" apps/<service>/src` | at least one consumer |

ALL FOUR must hold for the var to actually reach the application. **Most-skipped layer is 6a** (compose YAML wire) — because layer 5 is the natural "I set the env" mental model and layer 7 is what the implementer wrote, but the bridge between them (compose YAML interpolation) is invisible if you don't go look for it.

### The fix template (for the next time this bites)

When Dokploy env is set but `docker exec X env` doesn't show it:

```yaml
# Open docker-compose.<name>.yml, find the service that needs the var, add ONE line:
service-name:
  environment:
    EXISTING_VAR_1: value
    YOUR_VAR: ${YOUR_VAR:-}    # ← add this; `:-` default so non-prod composes still come up
```

PR the change, merge, auto-deploy fires, container rebuilds with the wire-through. Re-probe layer 6b to close. Same shape as the existing `OPENAI_API_KEY: ${OPENAI_API_KEY:-}` and `FEATURE_ADMIN_IMPERSONATION_ENABLED: ${FEATURE_ADMIN_IMPERSONATION_ENABLED:-false}` patterns in `monorepo-incluir/docker-compose.new-app.yml`.

---

## 13. Canonical prod public subdomains (incluir stack)

The hono-fed Next.js multi-app stack on prod uses **short subdomain prefixes** that don't always match the app folder name. When constructing layer-8 verify probes (`curl https://<host>/...`), use the table below — using the long form (e.g. `gestao-de-pessoas-new`) hits a Traefik 404 and wastes a probe.

| App folder | Container suffix | **Public subdomain** | Verify probe |
|------------|------------------|----------------------|-------------|
| `apps/frontend` (legacy admin/student) | `frontend-1` | **`new.programaincluir.org`** | `curl -I https://new.programaincluir.org/home/admin` |
| `apps/secretaria` | `secretaria-1` | **`secretaria-new.programaincluir.org`** | `curl -I https://secretaria-new.programaincluir.org/alunos/buscar` |
| `apps/gestao-de-pessoas` | `gestao-de-pessoas-1` | **`gestao-new.programaincluir.org`** ⚠️ (short form) | `curl -I https://gestao-new.programaincluir.org/voluntarios/buscar` |
| `apps/coordenador` | `coordenador-1` | `coordenador-new.programaincluir.org` (verify before use) | — |
| `apps/financeiro` | `financeiro-1` | `financeiro-new.programaincluir.org` (verify before use) | — |
| `apps/hono-app` | `hono-app-1` | NOT publicly routed — accessed via the per-app Next.js proxies above | — |

**Gotcha banked 2026-05-26**: orchestrator briefed Peppy with `gestao-de-pessoas-new.programaincluir.org` for the #431 verify chain. Real Traefik host is the short form `gestao-new.programaincluir.org`. Peppy caught it and re-probed; verify chain came back green. Container names use the full app folder name (`gestao-de-pessoas-1`), but Dokploy's domain config maps the short prefix.

**Discovery procedure** when you don't know an app's subdomain:
```bash
# List all Traefik hosts on the active compose
ssh xerox 'docker exec dokploy-postgres.1.zos6qj3u1fm7t10d72r5yzpc0 psql -U dokploy -d dokploy -t -c \
  "SELECT host FROM domain WHERE \"composeId\" = '\''_A6rI-GEm9oF8ysIojm0O'\'' ORDER BY host;"'
```

This returns every public host bound to the active Incluir Main App compose. Always run this discovery once at the start of a verify chain if the URL isn't already banked above.
