# communicate — precedents & protocol catalogue

Companion to `SKILL.md`. The rules stay there; this file holds the banked stories that established them and the per-stack verify protocol catalogue. Every block below is verbatim from the pre-split `SKILL.md`, filed under the section it came from.

---

## §7.1 lz9y — evidence-attached doorbell rule

**Banked precedent: lz9y AI intake, 2026-05-23 — three premature "feature live" claims to operator in 90 minutes, each one wrong at a different layer.**

---

## §7.2 anti-patterns — verifying layer N+1 and inferring layer N

From §7.2 "The trap to avoid". Examples of this anti-pattern:
- Checking the env var is set on the running container (layer 5) and inferring the build-time-inlined client bundle has it (layer 3) — the container has the var but the already-built bundle doesn't. Banked 2026-05-23 (Next.js NEXT_PUBLIC_).
- Checking the package is published to the registry (layer 4) and inferring downstream apps will resolve it (layer 8) — peer-deps or lockfiles can pin the old version.
- Checking the container is running (layer 6) and inferring the route exists (layer 8) — a stale image can be running fine while missing the new route.
- Checking the new column exists in the DB (layer 5) and inferring the code that uses it ships in the same deploy (layer 6) — schema + code can drift.
- Checking the API responds (layer 8) and inferring the auth gate resolves correctly (layer 7) — a permissive default can mask a broken gate.

---

## §7.2.1 Example protocol catalogue (extend per project)

The general principle is universal; the specific probes are project-specific. Start from your project's existing patterns; add a protocol the first time you ship that kind of feature, refine it the next time. Three reference examples below — replace / extend with the protocols your stack actually needs.

**Example A — Web app feature behind a flag (Next.js + Docker + Dokploy, monorepo-incluir):**
1. PR merged → SHA + merge time
2. Build args declared in Dockerfile → `grep "ARG NEXT_PUBLIC_FLAG" Dockerfile` (only NEXT_PUBLIC_ vars need build-arg wiring; server-only env vars skip this)
3. Build args passed via docker-compose → `grep "build.args" docker-compose.yml | grep FLAG`
4. Deploy completed → container restart timestamp
5. Runtime env present (server-only flags) → `docker exec X env | grep FLAG`
6. Build-time bake present (NEXT_PUBLIC_ flags) → `curl prod/_next/static/chunks/*.js | grep "FLAG":"true"` — the canonical layer-3 probe
7. Route responds with auth → `curl prod/page → 200`
8. Sidebar/nav surface renders → bundle-grep OR authenticated screenshot

**Example B — Backend API endpoint (any HTTP service):**
1. PR merged
2. Deploy reached the running service (restart timestamp or revision id)
3. Endpoint exists → `curl -I prod/api/route` → expected method-allowed status (not 404)
4. Endpoint with auth → `curl -X POST -H "auth: ..." -d '...' prod/api/route` → expected payload shape
5. Downstream side effects → live query of the DB / queue / log that should reflect the action

**Example C — Library / SDK release (any package registry):**
1. Version tag pushed
2. Package published → registry shows the version (`npm view @org/pkg versions`, `cargo search`, `pip show`, etc.)
3. Downstream consumer can resolve → in a fresh project: install the version, import a known-new symbol
4. Downstream consumer's lockfile updated (peer-dep / engines / minimum-version constraint resolves correctly)
5. Smoke test exercising the new symbol passes in the consumer

**For your project's other feature kinds** — mobile app store release, native binary distribution, CLI tool, browser extension, scheduled job, message-queue consumer, IaC change, DNS change, etc. — file the protocol the FIRST time the feature kind ships, in this catalogue, with the same shape: layers + per-layer probe + canonical artifact. Future agents can then run the existing protocol instead of re-deriving it.

---

## §7.3 lz9y recon — verify against origin/main, not your local checkout

**Banked precedent: lz9y AI intake recon, 2026-05-23 — orchestrator grepped local working tree and concluded "frontend doesn't exist," then filed 3 duplicate beads as if greenfield. Vance's subagents caught the duplication, but only after wasted dispatches.**

Closing sentence of §7.3 (followed the ~1-hour staleness rule):

The same recursion applies that Cipher and Atlas codified: "verify against reality" needs to be applied at the RIGHT artifact layer — local-stale-clone is not the reality you're claiming about.

---

## §7.4 eunenem 26wof — route operator-judgment questions through GLaDOS

**Banked precedent (2026-07-27, eunenem product-catalog spec, aperture-26wof):** Wheatley hit a genuine ambiguity mid-recon (what does "product lists" mean — kits, categories, or both?) and surfaced it with a local interactive multi-choice prompt that blocked his own turn on his own tmux pane. That design assumes someone is physically attached to *his* window at that exact moment — nobody is, by default. The operator only learned the question existed because they separately asked GLaDOS "is Wheatley stuck?" and GLaDOS deep-peeked his pane manually (per `agent-liveness`). Had that prompt not come, the question could have sat blocked indefinitely with nothing signaling either GLaDOS or the operator that input was needed.
