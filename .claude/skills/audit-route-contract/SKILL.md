---
name: audit-route-contract
description: How to detect frontend HTTP calls that point to non-existent backend routes — the silent-404 bug class that survives backend migrations, route refactors, and incremental rewrites. Use when investigating a 404 on a feature "that used to work," when migrating a backend (Mongoose → Hono, REST → GraphQL, monolith split), or proactively after any merge that renames or removes a route. Triggers on phantom routes, silent 404, route contract drift, frontend-backend URL mismatch, "I clicked save and nothing happened."
---

# Audit Route Contract

The bug: **the frontend calls `PATCH /api/students/by-user-class`, the backend has no such route, and every save in the matriculas dialog 404s in production for months until a real user notices.** No exception is thrown, no 500, no test fails — the network tab shows a clean 404 response, the action's `catch` block returns a generic toast, and the operator stares at "Erro ao salvar" with no idea why.

This is not a bug-in-isolation. It's a **contract drift** between two layers that look fine in isolation: frontend code compiles, backend code runs, lockfile is happy, CI green. The drift only surfaces when a real request hits the wire.

This skill is the rule for catching it before users do.

---

## 1. The War Story

On 2026-05-09 the operator hit a 404 saving an enrollment in `apps/frontend` matriculas. Trace `f6ac81f7e88d67967d09688cc9210281` showed `PATCH /api/students/by-user-class` returning 404. The frontend file calling it (`apps/frontend/src/backend/students/putStudent.ts`) was committed as part of the original Mongoose backend; when the migration to Hono happened, `enroll` and `:id/status` got Hono routes, but `by-user-class` (which lets the UI update enrollment metadata by `(userId, classId)` instead of by row UUID) was just… not built. The frontend kept calling the path. Hono kept 404'ing. Nobody noticed because:

- TypeScript types matched (`createAPIMethod<EnrollmentOptions, IStudent>` is happy with any URL string)
- Unit + integration tests cover the routes that exist, never the ones that don't
- The action's `catch` block converted "HTTP 404" into a toast string the user couldn't decode
- The toast wasn't disruptive enough for users to file tickets — they just retried, gave up, and worked around it

After fixing the immediate report, an audit (regex frontend `createAPIMethod` URLs vs Hono `app.route(...)` mounts × `app.METHOD(...)` paths) found **10+ more** phantom routes in the same codebase. Same root cause: backend migration left the frontend ahead of the actual API surface.

**The dumb part:** the audit was 30 lines of Python. No magic. Anyone could have run it the day the migration shipped.

---

## 2. The Forensic Signature

Spot this in code review or self-review by looking for any of:

| Smell | What it usually means |
|---|---|
| Frontend HTTP client takes a `url` as a string template | Compiler can't typecheck route existence; only runtime knows |
| Backend has migrated stack/framework in last N months | Half the frontend predates the migration; the routes that "worked before" still exist as call sites without server counterparts |
| Action `catch` blocks return generic toast copy | The action is swallowing real status codes — including 404 — into one indistinguishable error message. You can't tell broken-route from broken-input |
| `gh search code "createAPIMethod" --repo X` returns more files than backend route definitions | Frontend has more endpoints than backend serves. Some of the delta is dead code; the rest is silent 404s |
| Recent commits mention "remove old endpoint" or "consolidate routes" without touching `apps/frontend/src/backend/**` | Author updated the backend, never grepped for clients of the removed path |
| Bug reports of the form "I clicked save and the page just refreshed" | Silent failure pattern. The 404 redirects (in some setups) or the action returns a generic error the UI ignores |

If you see two or more of these on a feature the operator is reporting broken, run the audit script (§4) before diving into the code.

---

## 3. The Rules

### Rule 1 — Audit immediately after any backend migration

A backend migration (Mongoose → Hono, Express → Fastify, monolith split, REST → GraphQL) is the highest-risk window for this bug class. The migration PR may add 30 routes; the previous backend served 50; the delta is 20 silent-404s waiting to be hit.

**Same PR as the migration:** include a contract audit — extract every frontend HTTP-call URL, cross-check against the new backend's route registry, and either (a) add the missing route, (b) remove the dead frontend caller, or (c) explicitly file a follow-up ticket per gap with operator approval.

If the migration PR doesn't list every gap, the migration isn't done.

### Rule 2 — Surface real HTTP status in the action toast

Don't let `catch (err) { return { error: 'Erro genérico' } }` swallow status codes. Wrap the API client so 404, 403, 401, 4xx, 5xx each propagate a distinct, human-readable message. The operator should see "Endpoint not found" on a 404 and "Forbidden" on a 403, not the same generic copy for both.

```ts
// ❌ Anti-pattern — every error looks the same
} catch (error) {
  console.error('updateX error:', error)
  return { success: false, error: 'Erro ao atualizar' }
}

// ✅ Real error reaches the toast
} catch (error) {
  console.error('updateX error:', error)
  return {
    success: false,
    error: error instanceof Error ? error.message : 'Erro ao atualizar',
  }
}
```

And the API client itself should pull the backend's structured `message` field, not just fall through to a generic `HTTP <status>`:

```ts
// ❌ Loses the backend's actual message
throw new Error(data.error ?? `HTTP ${res.status}`)

// ✅ Surfaces the backend's `message` field first
throw new Error(data.message ?? data.error ?? `HTTP ${res.status}`)
```

This doesn't fix the contract drift — it makes the bug self-diagnosing when it surfaces.

### Rule 3 — Pin the contract with a smoke test

For every frontend HTTP call site, add at minimum a smoke test that asserts the backend route exists. The test doesn't need to exercise the full happy path — it just needs to send a malformed request and assert the response is **not 404**:

```ts
test('PATCH /api/students/by-user-class is registered', async () => {
  const res = await app.request('/api/students/by-user-class', {
    method: 'PATCH',
    headers: { Cookie: validAuth() },
    body: JSON.stringify({}),
  });
  // We don't care if it 400s on validation. We DO care that
  // the route exists. 404 here means contract drift.
  expect(res.status).not.toBe(404);
});
```

The audit script (§4) generates this list automatically. Pipe it into a test file, commit, never regress.

---

## 4. The Audit Script

This is the meat of the skill. Run it any time you suspect contract drift; run it as part of any backend-migration PR's CI.

```python
#!/usr/bin/env python3
"""Cross-reference frontend HTTP call sites vs backend route registry.
Prints every (method, url) pair the frontend calls that has no matching
backend route. Adapt the regex / paths to your stack.

Tested against: Hono backend with `app.route('/api/foo', fooRoutes())`
mounts, `app.METHOD('/path', ...)` route definitions.
Frontend with `createAPIMethod<X, Y>({ method: 'PATCH', url: '/api/foo' })`.
"""
import re
from pathlib import Path

# ── Frontend call extractor ─────────────────────────────────────────────
api_pat = re.compile(
    r"createAPIMethod\s*<[^>]*>\s*\(\s*\{"
    r"\s*method:\s*['\"](\w+)['\"]"
    r"\s*,\s*url:\s*[`'\"]([^`'\"]+)",
    re.DOTALL,
)

# ── Backend mount + route extractors ────────────────────────────────────
mount_pat = re.compile(
    r"app\.route\(\s*['\"]([^'\"]+)['\"],\s*(\w+)\("
)
hono_pat = re.compile(
    r"app\.(get|post|put|patch|delete)\(\s*['\"]([^'\"]+)['\"]"
)

# 1. Find all backend route mounts: `/api/foo` -> functionName
app_ts = Path('apps/hono-app/src/http/app.ts').read_text()
mounts = list(mount_pat.finditer(app_ts))

# 2. Build function -> file map for the route modules
fn_to_file = {}
for f in Path('apps/hono-app/src/http/routes').glob('*.ts'):
    txt = f.read_text()
    for m in re.finditer(r'export function (\w+)\(', txt):
        fn_to_file[m.group(1)] = f

# 3. Collect all backend routes as (METHOD, canonical-path) tuples
hono_routes = set()
for m in mounts:
    base, fn = m.group(1), m.group(2)
    f = fn_to_file.get(fn)
    if not f:
        continue
    for hm in hono_pat.finditer(f.read_text()):
        path = hm.group(2)
        full = (base + (path if path != '/' else '')).rstrip('/') or '/'
        canon = re.sub(r':\w+', ':param', full)
        hono_routes.add((hm.group(1).upper(), canon))

# 4. Walk frontend, extract every createAPIMethod URL
missing = []
for f in Path('apps/frontend/src/backend').rglob('*.ts'):
    txt = f.read_text()
    for m in api_pat.finditer(txt):
        method = m.group(1)
        url = m.group(2).split('?')[0]
        if not url.startswith('/api/'):
            continue  # skip absolute URLs to other services
        # Replace template-literal interpolations with :param for matching
        canon = re.sub(r'\$\{[^}]+\}', ':param', url).rstrip('/') or '/'
        if (method, canon) not in hono_routes:
            missing.append((method, canon, str(f).split('frontend/')[-1]))

# 5. Filter framework-handled paths if applicable (BetterAuth, etc.)
FALSE_POSITIVES = {
    ('POST', '/api/auth/forget-password'),
    ('POST', '/api/auth/reset-password'),
    ('POST', '/api/auth/sign-up/email'),
}
real = [m for m in missing if (m[0], m[1]) not in FALSE_POSITIVES]

print(f'Phantom routes: {len(real)}')
for method, url, src in real:
    print(f'  {method:6} {url}  ({src})')
```

**What you do with the output:**

For each phantom route, exactly one of:

1. **Build the missing backend route.** The frontend feature is real and used; the backend never got the endpoint. Ship the route + auth + tests.
2. **Delete the frontend caller.** The feature was removed; the call site is dead code that's been silently 404'ing for months. Remove it; remove its UI button if applicable.
3. **File a ticket with operator approval.** You know it's a gap, you don't have time to fix it now, the operator agrees the feature can stay broken until N. The ticket exists so future-you doesn't re-discover it via a P0.

**Never** silently ignore an entry. Each gap is a feature that doesn't work; pick the disposition that matches reality.

### When the same business flow surfaces phantom in N apps — canonicalize, don't parallel-fix

When an audit surfaces the **same business flow** (e.g., "transfer a student between classes") with **different phantom paths in different apps** — e.g., `apps/frontend` callers use `POST /api/students/transfer` and `apps/secretaria` callers use `PATCH /api/students/:id` with `{ classId }` body — the disposition isn't "fix each app's URL independently." It's **N+1 work items: build/canonicalize the one true endpoint, then update each app's caller to use it.**

Two parallel fixes lock the swarm into two slightly-different "transfer student" implementations forever. Each gets its own backend route, each accumulates its own divergent edge-case handling, each becomes the surface a future bug lands on. The repo ends up with `POST /api/students/transfer` AND `PATCH /api/students/:id` AND a bulk-transfer endpoint added by someone who didn't notice the first two — three implementations of the same user-visible behaviour, none authoritative.

The audit catches this pattern when classifying RED phantoms. Look for:
- Multiple RED entries that semantically describe the same operation (same nouns + verbs + intent)
- Different URL shapes across the apps (path-param vs query-param vs body-keyed)
- Same payload field set (modulo naming) on each phantom request

When you spot this in the audit table, file ONE coordination bead — "design the canonical endpoint; update both/all callers to use it" — not N independent bug beads. The audit's job is to surface this clearly enough that the implementer can't accidentally pick the parallel-fix path. Real precedent: aperture-tpxu surfaced `POST /api/students/transfer` phantom (apps/frontend); aperture-vdw2 surfaced `PATCH /api/students/:id` phantom (apps/secretaria) — same business flow, two phantom paths, filed as aperture-8twp + aperture-xezc with cross-references in both bead bodies pointing at each other so Rex catches the coordination requirement before he picks up either ticket.

---

## 5. Code Review Checklist

When reviewing a PR that:
- migrates the backend stack
- renames a route (`/foo` → `/foo/v2`)
- removes a route
- adds a new frontend HTTP call site

Ask **all** of these. Don't approve until each is answered with a concrete file:line reference, not "we'll add it later."

- [ ] **For every removed/renamed backend route: where are the frontend callers updated?** (`grep -rn "<old-path>" apps/*/src/`)
- [ ] **For every new frontend `createAPIMethod` URL: which backend route handles it?** (file:line)
- [ ] **For migrations: was the audit script (§4) run? Where's the output?** (paste in PR description)
- [ ] **Does the frontend's API client surface the backend's `message` field on errors, or does it generic-string-fall-through to "HTTP 404"?**
- [ ] **Is there a smoke test (per §3 Rule 3) for the new route?**

If you can't tick all five, the PR ships at least one silent-404. No exceptions for "it's just a route rename" or "the tests cover it" — unit/integration tests don't catch path drift.

---

## 6. The Forensic Drill (3-minute diagnosis)

When a user reports "I clicked save and nothing happened" or you see a 404 in DevTools that shouldn't exist:

1. **Find the request in DevTools** — note method + URL + status
2. **Grep backend for the path** — `grep -rn "<path>" apps/<backend>/src/http/routes/`
3. **If grep returns ≤ 0 hits or nothing matching the method → confirmed phantom route.** Skip to remediation.
4. **If grep returns hits, check the auth middleware** — maybe it's a 403 masquerading as something else. (The `requirePermission` shape in this repo throws ForbiddenError, not 404, but other middleware patterns can produce a 404 from a missing auth lookup.)
5. **Remediation:** decide between §4's options 1, 2, or 3.

This is a 3-minute diagnosis once you know the shape. Faster than reading the action code, faster than re-running tests, faster than redeploying.

---

## 7. Why This Class Is Particularly Insidious

Most "tests pass but prod broken" bugs have a smell: missing wiring (wire-the-adapter), missing env var (config drift), missing migration (schema drift). All of those throw at runtime — 503, undefined-property, "relation does not exist."

Phantom routes don't throw. They return a clean HTTP 404 — a valid response that just doesn't do what the caller wanted. Every layer is happy:

- Network stack: 404 is a normal HTTP code
- HTTP client: returns a normal `Response` object with `ok: false`
- Action handler: catches the `Error("HTTP 404")`, returns generic toast
- UI: shows the toast, user retries, retries, gives up

The bug is invisible to every observability layer except a real user clicking and waiting. **This is why the smoke test in §3 Rule 3 is non-negotiable.** Compile-time can't catch it (URLs are strings). Integration tests don't catch it (they exercise routes that exist). Only a request that asserts "this path is registered" catches it.

---

## 8. Anti-Patterns to Reject in Review

| Anti-pattern | What to say |
|---|---|
| "I renamed the backend route, the frontend will get updated in a follow-up" | "No — same PR. Follow-ups die; the feature ships broken in the meantime." |
| "The route is missing because that feature is being deprecated" | "Then delete the frontend caller in this PR. Don't let it sit and rot." |
| "We can add the route later when someone needs it" | "Someone DOES need it — they're hitting a 404 right now. Either build it or remove the UI surface." |
| "We have integration tests, they would have caught this" | "Integration tests cover routes that exist. They cannot catch a route that doesn't exist. You need a smoke test asserting `not 404`." |
| "It's just a frontend `createAPIMethod` URL, low risk" | "It IS a contract. The frontend declares 'this endpoint exists.' If it doesn't, every UI button for that feature is a 404 generator." |

---

## 9. GET/POST Sibling-Endpoint Shape Parity

A related-but-distinct bug class to phantom routes: **the route exists, but its response shape silently disagrees with a sibling route on the same entity.** The frontend gets a 200 OK with the wrong shape, sometimes silently drops fields, sometimes shows a misleading toast ("Resposta inesperada do servidor (sem dados do voluntário)"). All paths are valid HTTP; the bug is in the contract.

### The recurring pattern

A route file has two handlers operating on the same conceptual entity:

- `GET /api/widgets/:id` — does a rich JOIN + builds a nested response (`{ id, widget: { ...joined fields... } }`)
- `POST /api/widgets/:id/actions` — inserts a row and returns a flat shape (`{ id, ...flat columns... }`)

The frontend reads BOTH endpoints to populate the same UI surface. It deserializes the GET shape correctly. It calls the POST and expects the same shape — but the POST handler shipped without the JOIN+serialization the GET does. Frontend reads `response.nested_field` → `undefined` → falls into the error branch.

### Banked precedents (2026-05-09 → 2026-05-27)

| Bead | Surface | Failure shape |
|---|---|---|
| `aperture-et45` | `GET /api/volunteers/:id` | Missing user JOIN; frontend wanted nested user data; got volunteer-only row |
| (banked earlier) | volunteer-related GET | Same family — JOIN missing |
| `aperture-q2az` | `POST /api/keys/:keyId/assignments` | GET returns nested `currentHolder`; POST returned flat `{assignmentId, keyId, ...}`; frontend `serializeHolder(undefined)` → error toast |

Three instances in <3 weeks. Time to bake the discipline.

### The rule

**When reviewing a route file with both GET and POST handlers returning the same conceptual entity, the POST response shape MUST be a strict subset-or-equal of the GET serialization** — OR the frontend MUST consume both responses through a shared shape-coercion helper that normalizes the difference.

The two acceptable shapes for a sibling pair:

**Shape parity (preferred):**
```ts
// GET handler
return c.json({ assignmentId, currentHolder: serializeHolder(joinedRow) });

// POST handler — IDENTICAL outer shape
return c.json({ assignmentId, currentHolder: serializeHolder(joinedRow) });
```

**Shared coercion helper (acceptable for legacy code):**
```ts
// Frontend
const holder = parseAssignmentResponse(response);  // handles both shapes
```

The forbidden shape:
```ts
// GET — nested
return c.json({ assignmentId, currentHolder: {...} });
// POST — flat (BUG)
return c.json({ assignmentId, volunteerId, name, ... });
```

### Review checklist for sibling endpoints

When auditing a route file:

1. **List every endpoint** that returns the same conceptual entity (by Zod schema name, by handler grouping, or by URL prefix).
2. **For each pair**, identify the response shape — manually if no schema, via the Zod response schema if present.
3. **Check shape parity** — do they have identical top-level keys? Identical nesting? If POST inserts a row, does it re-fetch + JOIN before returning, or does it return the bare insert?
4. **Trace frontend consumption** — does the frontend pass both responses through the same parser (e.g. `serializeHolder`, `parseWidget`)? If yes, the parser is the contract — both endpoints must produce shape the parser accepts. If no, every endpoint is its own contract.
5. **Red-flag indicators**:
   - POST handler that returns `c.json({ ...insertedRow })` without a follow-up SELECT
   - POST handler that returns `c.json({ id, ...input })` echoing input + the generated id only
   - Sibling GET handler with a visible JOIN or serializer call that POST doesn't have

### Why this matters more than it looks

These bugs are invisible to TypeScript (the response type is often `unknown` after the network boundary), invisible to CI (the backend test asserts insert worked, the frontend test mocks the response), and invisible to a deployed smoke test (the 200 OK passes "the route exists" check). Only an operator clicking the actual UI catches them — same trap as phantom routes, different surface.

If you ship a new POST handler, **ship its sibling-endpoint shape check in the same PR**. Don't trust "the GET already works, surely the POST returns the right thing."

---

## 10. Closing Thought

Backend migrations are the highest-risk window for this bug class — and the easiest to audit. The script in §4 is 30 lines. It runs in seconds. It will tell you, deterministically, every place where the frontend has gotten ahead of the backend's actual surface.

If you find yourself merging a backend-migration PR without running it, stop. Run it. Fix the gaps in the same PR. Future-you will not remember to come back. CI will not catch them. Code review may not catch them. The next person to find each one will be a real user, in production, watching their click do nothing — exactly like the operator on 2026-05-09.

Don't let that be how the work ships.
