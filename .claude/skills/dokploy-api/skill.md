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

### Incluir Org (Main App, Infra, Waha)
```bash
TOKEN="lZMOoQglUNzSTzYMtUBtOPHQsRridBcOefVZDjDSEQtZGpBjtkugbjVjJXQqTJBH"
```

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
| Service | Compose ID |
|---------|-----------|
| Main Apps (Prod) | `4sHHtg1XwERiDc6o2labm` |
| Minio | `biqK8MbgAXtrJH24k5zTg` |
| Unleash | `27vJsrYScdmCcKf1qVh6Y` |
| Waha | `uIBU4__1Jw3RGp6WSzz6y` |

---

## 10. Safety Reminder

| Tier | Operations | Rule |
|------|-----------|------|
| **Read-only** | `.one`, `.all`, project-list, compose-info | Run freely |
| **Operational** | `.create`, `.deploy`, `.update`, `.stop`, `.start` | Operator approval required |
| **PROHIBITED** | `.delete`, `.remove` | Never. No exceptions. |
