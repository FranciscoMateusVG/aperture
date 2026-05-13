---
name: sentry-query
description: Query Sentry for production error/issue data via REST API + curl while the official MCP work (aperture-t3q9) lands. Use when an operator surfaces a Sentry issue URL ("https://xerox-to-xerox.sentry.io/issues/<id>/"), when you need the exception type / stack trace / affected users for a reported bug, when you're triaging an incident and need to read the latest event payload, or when you're filtering issues by environment / user / release. Triggers on "sentry", "sentry issue", "sentry.io", "xerox-to-xerox.sentry", "issue 7479…", "stack trace from prod", "what's the error", "exception in prod", "affected users", "Sentry triage", "Sentry event", "SENTRY_AUTH_TOKEN", ".sentryclirc".
---

# Sentry Query — Bridge skill (curl + REST API)

This skill is the **agent-callable access pattern** for Sentry while the proper MCP work in `aperture-t3q9` is being built. Same skill name (`aperture:sentry-query`), same agent muscle memory; only the implementation underneath swaps when the MCP lands.

Org slug: **`xerox-to-xerox`** (per https://xerox-to-xerox.sentry.io/). API base: **`https://sentry.io/api/0/`**.

## When to use

- Operator pastes a Sentry issue URL and asks "what's going on here?"
- A user reports a bug and you want to see if Sentry already captured the exception (faster than re-running the failing flow)
- You have an `issue_id` (last path segment of a Sentry issue URL) and want the latest event with full stack trace
- You're hunting all errors for a specific user, environment, or release
- You're checking whether a recent deploy regressed (filter `release:` or `firstSeen:>...`)

If the answer might come from Tempo traces or Loki logs instead, see `observability-query`. If from Postgres data, see `incluir-prod-postgres`.

## Access pattern

The dedicated agent-query token lives on **xerox** at `~/.config/aperture/sentry-agent-token` (chmod 600, owned by the `ubuntu` user). Don't replicate it locally — fetch it on demand via ssh. This keeps a single source of truth, lets Peppy rotate without touching agent shells, and works from any machine an Aperture agent runs on.

**Verified scopes** (Peppy, 2026-05-13): `event:read`, `project:read`, `org:read`, `member:read`, `team:read`. No write. If a recipe here ever needs a write endpoint, **stop** and BEADS-task Peppy — we mint a separate scoped token rather than expand this one.

Resolve the token + standard env at the top of any recipe:

```bash
TOKEN=$(ssh xerox 'cat ~/.config/aperture/sentry-agent-token')
[ -z "$TOKEN" ] && { echo "ERROR: empty token from xerox" >&2; exit 1; }

ORG=xerox-to-xerox          # org id 4506836544847872
API=https://sentry.io/api/0
```

**DO NOT** echo, log, or paste the token value. Don't store it in BEADS notes, don't include it in `close_reason`, don't bake it into example shell snippets — only ever the variable name `$TOKEN`. The mempalace mirror is in `peppy/secrets` (drawer `peppy_secrets_92eb12e845b8ab31217ea091`) for provenance / rotation history.

**Sanity check** (always your first call in a new session — confirms ssh hop + token + scope all work):

```bash
curl -sS -H "Authorization: Bearer $TOKEN" "$API/organizations/$ORG/" \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); print("ok:", d["slug"], "id="+str(d["id"]))'
# Expect: ok: xerox-to-xerox id=4506836544847872
# 401 = token bad/rotated — message Peppy. 403 = scope missing — message Peppy. 404 = wrong slug — typo.
```

## Project IDs lookup

Sentry's REST API accepts project IDs (numeric) **or** project slugs (string) in most endpoints, but several search endpoints require the numeric ID. Cache them once per session:

```bash
curl -sS -H "Authorization: Bearer $TOKEN" "$API/organizations/$ORG/projects/" \
  | python3 -c '
import json, sys
for p in json.load(sys.stdin):
    print("%-22s %-30s %-25s %s" % (p["id"], p["slug"], p.get("platform","-"), p["name"]))'
```

**Bake-in table** (validated 2026-05-13 against `xerox-to-xerox`):

| Project | Slug | Numeric ID | Platform |
|---|---|---|---|
| Incluir (Next.js + hono-app, single Sentry project) | `incluir` | `4511379852296192` | `javascript-nextjs` |

> **Single-project setup as of 2026-05-13.** The whole monorepo currently reports under one `incluir` project — both the frontend Server Actions/SSR layer (node platform) and any future browser-SDK telemetry land in the same Sentry project, distinguished by the event's `platform` and `tags.runtime`. EuNeném v2 does **not** have a Sentry project yet; if/when it does, add a row here in the same PR that wires up the SDK.
>
> Re-run the lookup curl above whenever a new project is added and update this table via PR. The table is the canonical reference; if it disagrees with what the API returns, the API wins and the table is stale.

## Recipe library

All recipes assume the `TOKEN`, `ORG`, `API` env vars from the access-pattern block above are set in the current shell.

### 1. Get an issue by ID (the "operator pasted a Sentry URL" recipe)

The numeric `ISSUE` id is the last path segment of the URL the operator pasted — e.g. `https://xerox-to-xerox.sentry.io/issues/7479030229/` → `7479030229`. The short id (`INCLUIR-5`) also works on most endpoints.

```bash
ISSUE=7479030229   # canary: INCLUIR-5 "APIError: Admin access required"
curl -sS -H "Authorization: Bearer $TOKEN" "$API/organizations/$ORG/issues/$ISSUE/" \
  | python3 -c '
import json, sys
d = json.load(sys.stdin)
for k in ("shortId","title","culprit","level","status","firstSeen","lastSeen","count","userCount"):
    print("%-12s %s" % (k+":", d.get(k)))
md = d.get("metadata") or {}
proj = d.get("project") or {}
print("project:     %s (id=%s)" % (proj.get("slug"), proj.get("id")))
print("metadata:    type=%s  value=%s" % (md.get("type"), md.get("value")))'
```

Returns the issue meta: `title`, `culprit`, `level`, `status`, `firstSeen`, `lastSeen`, `count`, `userCount`, `project.slug`, `metadata.type`, `metadata.value`. Good for triage at a glance — but **always follow with Recipe 2** (latest event) for the actual stack + breadcrumbs.

### 2. Get the LATEST event for an issue (full stack trace) — the headline recipe

You almost always want this when triaging. Returns ~40-200KB of JSON; **always** pipe through the pretty-printer at the bottom of this file (or a tight `jq` filter). Never dump raw to your context window.

```bash
ISSUE=7479030229
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$API/organizations/$ORG/issues/$ISSUE/events/latest/" \
  | python3 inspect-sentry-event.py
```

The pretty-printer surfaces: exception type + message, top in-app stack frames (file:line), recent breadcrumbs (HTTP requests, console logs, navigation), tags (environment, release, browser, runtime), user context, and the `trace_id` from `contexts.trace`.

**The `trace_id` is gold.** Once you have it, jump to `observability-query` and pull the full Tempo waterfall:

```bash
TRACE_ID=85b86674dae443ba9180c59ed16fcd01    # from contexts.trace.trace_id in the event
ssh xerox "docker exec compose-override-solid-state-port-349ude-hono-app-1 \
  sh -c 'wget -qO- http://incluir-tempo:3200/api/traces/$TRACE_ID'" \
  | python3 inspect-trace.py    # the observability-query pretty-printer
```

This is the canonical Sentry → Tempo pivot — front-end Sentry tells you "what broke," Tempo shows the full server-side waterfall around the failure.

### 3. List events for an issue (paginated)

```bash
ISSUE=7479030229
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$API/organizations/$ORG/issues/$ISSUE/events/?per_page=10" \
  | python3 -c 'import json,sys; [print(e["id"], e.get("dateCreated"), e.get("user",{}).get("email","-")) for e in json.load(sys.stdin)]'
```

### 4. Search issues with a query

URL-encode the query string. Examples:

```bash
# All unresolved issues in the last 24h on the incluir project
Q='is:unresolved age:-24h'
ENCODED=$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))" "$Q")
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$API/organizations/$ORG/issues/?query=$ENCODED&project=4511379852296192&statsPeriod=24h" \
  | python3 -c 'import json,sys; [print(i["shortId"], i["count"], i["title"][:80]) for i in json.load(sys.stdin)]'

# All issues touching a specific user (by email)
Q='user.email:someone@example.com'
ENCODED=$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))" "$Q")
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$API/organizations/$ORG/issues/?query=$ENCODED&statsPeriod=30d"
```

Common query keywords: `is:unresolved`, `is:resolved`, `level:error`, `environment:production`, `release:<sha>`, `firstSeen:>YYYY-MM-DD`, `user.id:<uuid>`, `user.email:<email>`, `transaction:/api/foo`.

### 5. Discover-style event search (lower-level than issue search)

For per-event filtering (each Sentry "issue" is a fingerprint group of N events — sometimes you want the events themselves, not the groupings):

```bash
Q='event.type:error user.id:<uuid>'
ENCODED=$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))" "$Q")
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$API/organizations/$ORG/events/?field=id&field=title&field=timestamp&field=user.email&field=transaction&query=$ENCODED&statsPeriod=7d" \
  | python3 -m json.tool | head -60
```

(Note: the legacy path `/eventsv2/` also works but is being deprecated. Use `/events/` going forward.)

### 6. List recent issues for a project (no query)

Useful for "what broke today on incluir?":

```bash
PROJECT_SLUG=incluir
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$API/projects/$ORG/$PROJECT_SLUG/issues/?statsPeriod=24h&query=is:unresolved&sort=date" \
  | python3 -c 'import json,sys; [print(i["shortId"], i["count"], i["lastSeen"], i["title"][:80]) for i in json.load(sys.stdin)]'
```

### 7. Get the affected-user list for an issue

```bash
ISSUE=7479030229
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$API/organizations/$ORG/issues/$ISSUE/tags/user/values/" \
  | python3 -c 'import json,sys; [print(v["count"], v.get("email") or v.get("identifier") or v["value"]) for v in json.load(sys.stdin)]'
```

### 8. Pagination

Sentry uses `Link:` response headers (RFC 5988) for cursors. Most triage queries fit in one page (default 100, max 100). When you need more:

```bash
curl -sSI -H "Authorization: Bearer $TOKEN" "$API/organizations/$ORG/issues/?query=is:unresolved" | grep -i '^link:'
# Look for: <...&cursor=0:100:0>; rel="next"; results="true"
```

Then re-issue the request with `&cursor=0:100:0` appended.

## inspect-sentry-event.py (inline pretty-printer)

Reads a Sentry event JSON from stdin, prints the bits a human-or-agent triager actually wants: exception type/message, top stack frames, tags, user, recent breadcrumbs, contexts (including `trace_id` for the Tempo pivot). Save once per session as `inspect-sentry-event.py` and pipe events through it.

Validated 2026-05-13 against issue 7479030229 (canary). On a ~40KB event payload it returns ~30 lines of triage-ready output.

```python
#!/usr/bin/env python3
"""Pretty-print a Sentry event payload from stdin."""
import json, sys

e = json.load(sys.stdin)
proj = (e.get("projectSlug")
        or (e.get("project") or {}).get("slug")
        or e.get("projectID")    # events/latest/ payload only has the numeric ID
        or "?")

print(f"== Event {e.get('eventID', '?')} | {e.get('dateCreated', '?')}")
print(f"   issue:       {e.get('groupID', '?')}  ({e.get('title', '')[:90]})")
print(f"   project:     {proj}    (cross-ref the bake-in table above for slug)")
print(f"   environment: {e.get('environment', '-')}    release: {e.get('release', '-') or '-'}")
print(f"   platform:    {e.get('platform', '-')}")

# Exception(s)
for entry in e.get("entries", []):
    if entry.get("type") == "exception":
        for exc in entry["data"].get("values", []):
            print(f"\n-- Exception: {exc.get('type', '?')}: {exc.get('value', '')[:200]}")
            frames = (exc.get("stacktrace") or {}).get("frames") or []
            print(f"   ({len(frames)} frames; showing app frames + bottom)")
            app_frames = [f for f in frames if f.get("inApp")]
            for f in (app_frames or frames)[-12:]:
                fn = f.get("function") or "?"
                file = f.get("filename") or f.get("absPath") or "?"
                line = f.get("lineNo") or "?"
                in_app = "★" if f.get("inApp") else " "
                print(f"   {in_app} {file}:{line}  in {fn}")
    elif entry.get("type") == "request":
        d = entry["data"]
        print(f"\n-- Request: {d.get('method', '?')} {d.get('url', '?')}")
    elif entry.get("type") == "breadcrumbs":
        crumbs = entry["data"].get("values", [])[-8:]
        print(f"\n-- Breadcrumbs (last {len(crumbs)} of {len(entry['data'].get('values', []))})")
        for c in crumbs:
            raw_ts = c.get("timestamp") or ""
            ts = raw_ts[:19] if isinstance(raw_ts, str) else str(raw_ts)[:19]
            cat = c.get("category", "?")
            msg = (c.get("message") or json.dumps(c.get("data") or {}))[:120]
            print(f"   [{ts}] {cat:>10}  {msg}")

# Tags
tags = {t["key"]: t["value"] for t in e.get("tags", [])}
interesting = ["browser", "browser.name", "runtime", "runtime.name", "os", "os.name",
               "transaction", "url", "user", "release", "environment", "level", "logger"]
print(f"\n-- Tags")
for k in interesting:
    if k in tags:
        print(f"   {k:>16} = {tags[k]}")

# User
user = e.get("user") or {}
if user:
    print(f"\n-- User")
    for k in ("id", "email", "username", "ip_address"):
        if user.get(k):
            print(f"   {k:>10} = {user[k]}")

# Contexts (browser, OS, runtime details)
ctx = e.get("contexts") or {}
if ctx:
    print(f"\n-- Contexts")
    for k, v in ctx.items():
        if isinstance(v, dict):
            short = ", ".join(f"{kk}={vv}" for kk, vv in v.items() if kk not in ("type",))[:200]
            print(f"   {k:>16}: {short}")
```

Usage:

```bash
ISSUE=7479030229
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$API/organizations/$ORG/issues/$ISSUE/events/latest/" \
  | python3 inspect-sentry-event.py
```

## Triage flow (the canonical "operator pasted a Sentry URL" walkthrough)

1. Pull issue meta (Recipe 1) — confirm title, project, level, count, first/last seen
2. Pull latest event (Recipe 2) through the pretty-printer — get exception type, message, top app frames, environment, user
3. Cross-reference if needed:
   - Affected users (Recipe 7) → is this one user or many?
   - For browser errors with a `trace_id` tag, jump to `observability-query` and pull the Tempo trace for the full server-side waterfall
   - For backend errors, check Loki logs around the `dateCreated` timestamp (`observability-query`)
4. Identify the triage owner:
   - Frontend stack frames → Vance
   - Backend stack frames (hono-app, API routes) → Rex
   - Auth-related (BetterAuth, session) → Cipher
   - Infra/deploy-correlated (immediately after a deploy) → Peppy
5. File a BEADS task for the fix; close the investigation task with the trace recorded.

## Safety tiers

| Tier | Operations | Rule |
|---|---|---|
| **Read-only** | `GET /organizations/.../issues/`, `/events/`, `/projects/`, `/tags/`, `/discover/` | Run freely. This is the entire bridge skill. |
| **Issue mutation** | `PUT /issues/<id>/` (resolve, ignore, assign), `DELETE /issues/<id>/` | Operator approval required. State note: by default the bridge token has `issue:read` ONLY — write attempts will 403. |
| **Project / org admin** | Anything under `/organizations/<org>/` that mutates settings, members, integrations, or scopes; project create/delete | PROHIBITED. File a BEADS task to GLaDOS. |
| **Source maps / artifact upload** | `POST /projects/.../files/source-maps/` | Belongs to Peppy's deploy pipeline, not this skill. |

If a recipe here ever returns 403, that's the scope working as designed — don't try to escalate the token without operator approval.

## Failure modes & gotchas

- **401 Unauthorized** — token missing, expired, or revoked. Re-run the sanity check; ask the operator to regenerate.
- **403 Forbidden** — token is valid but scope is missing for this endpoint. The bridge token's scopes are `event:read`, `project:read`, `org:read`, `issue:read` — anything else (write, delete, admin) will 403 by design.
- **404 on issue ID** — either wrong ID, wrong org, or the issue was deleted. Sanity check the URL the operator pasted; the issue ID is the LAST path segment after `/issues/`.
- **Rate limits** — Sentry returns 429 with `X-Sentry-Rate-Limit-Remaining: 0`. Default is generous (~40 req/sec for User Auth Tokens) but if you're paginating a huge query, sleep between pages.
- **Huge event payloads** — a single event with deep stack + breadcrumbs can be 50–200KB of JSON. ALWAYS pipe through `jq` or the pretty-printer; never dump raw to your context window.
- **`projectSlug` vs `project.slug`** — different endpoints return the slug at different keys. The pretty-printer handles both; if you write your own jq filter, check first.
- **Quoted special chars in queries** — `query=user.email:foo@bar.com` is fine, but anything with spaces (`message:"some thing"`) needs full URL encoding via the python helper, not a naked curl with shell escaping.

## Cross-references

- **`observability-query`** — Tempo + Loki access. Use AFTER pulling a Sentry trace_id tag to get the full server-side waterfall for the same trace.
- **`incluir-prod-postgres`** — When the Sentry event names a user UUID and you want their account state.
- **`mongo-query`** — Same for Mongo-backed data (chat, etc.).
- **`audit-route-contract`** — When the Sentry exception is a 404 from the frontend hitting a removed/renamed backend route.

## Migration note

This skill currently uses **curl + REST API** as a bridge while **`aperture-t3q9`** (P1 epic) wires up the official Sentry MCP server with per-agent OAuth. When that lands, the recipes here will be rewritten to use `mcp__sentry__*` tool calls — same skill name, same agent muscle memory, same triage flow, much less ceremony.

The skill name (`aperture:sentry-query`) is the stable contract. The implementation underneath is allowed to evolve. If you're reading this and `mcp__sentry__*` tools are available in your tool list, **use those instead of the curl recipes here** and file a follow-up to delete this scaffold.

Tracking: `aperture-vzuu` (this bridge) → `aperture-t3q9` (the proper MCP epic).
