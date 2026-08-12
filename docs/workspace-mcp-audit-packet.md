# Workspace MCP audit — improvement decision packet

**Author:** Wheatley (planning) · **Executor:** Peppy · **Date:** 2026-08-09
**Target:** `mini:~/projects/mcp-for-claude`. Audit of the non-WhatsApp, non-storage tools (Gmail, Calendar, Contacts, Tasks, StarkBank). Source-read on the live Mini via 3 parallel audits. Ranked value-to-effort. **NOT a rewrite — a shortlist to pick from.**

---

## ⛔ GATE (rides above everything) — StarkBank money-safety review
The StarkBank surface (dedicated AI banking account, **production**, real money) has **zero guardrails**: no confirmation/preview, no amount cap, no idempotency, no audit trail, plus an **active booby-trap** (the retry wrapper auto-double-pays the instant its error classification is "fixed"). **Escalated separately to GLaDOS for a Cipher security review; that review GATES any StarkBank feature work.** Details in the escalation. Do NOT build favorites/scheduling/status-polling/statement-filtering on that surface until idempotency + confirm + caps + audit land (filed-p2-becomes-prereq). The feature findings below are Gmail/Calendar/Contacts/Tasks ONLY.

---

## 🔧 CORRECTION to the round-2 finding (source-verified — I got this wrong before)
Round-2 (and my FYI to GLaDOS/Peppy) said *"the Google Tasks tools are a confirmed live 403 bug — no tasks scope."* **That was a mis-diagnosis.** Corrected against the live source + a re-verified tokeninfo:
- **The "Tasks" tools do NOT call the Google Tasks API at all** — grep for tasks_v1/google.tasks/auth/tasks = zero hits. They're a **hack over Calendar**: `create-task` inserts a flamingo-colored 1–5 AM event, `get-tasks` filters today's flamingo events, `complete-task` recolors to graphite. They ride the **calendar** scope, so they never 403 on a "tasks scope." They *work* — badly (see below).
- **The live 403 Peppy saw** was a *direct probe of the Tasks API* (`GET tasklists` → 403) — correct for that API, but the tools don't use it. We both assumed they did. Drive's 403 was real and still stands (Drive is a genuine new-feature scope gap — but Drive was dropped in round 3 for local storage anyway).
- **Contact WRITES are authorized, not broken.** Live token has **full `https://…/auth/contacts`** (re-verified), so create/update/delete-contact are scope-authorized. ⚠️ LATENT TRAP: the mint script `scripts/get-refresh-token.js` now requests `contacts.readonly` — so if anyone re-mints the token from the current script, contact writes would NEWLY break. Worth fixing the script to match the live grant.
- **Net: NO tool on the Google surface is 403-broken by scope today.** The only scope gaps are for things not yet built (Drive) or a Tasks-API rewrite (below).

---

## Gmail (6 tools) — scope is `mail.google.com` (full); every add below is already authorized, NO re-consent

**Ranked adds:**
1. **Send a NEW email** — `messages.send`. VERY HIGH value / VERY LOW effort. The machinery already exists inside `reply-email` (encodeBase64Url + messages.send + MIME builder). A PA Gmail that can only reply-never-compose is the biggest gap. **Top pick.**
2. **Modify state: mark-read / archive / mark-unread / star / important** — `messages.modify` (add/remove labels). VERY HIGH / VERY LOW. One repo fn covers all five. Today you **can't even mark a message read** — `read-email` doesn't, and nothing else does either.
3. **Return structured IDs/thread/labels in tool output** — formatting only, HIGH / VERY LOW. Today the structured Email[] is computed but only emitted as emoji prose (agents screen-scrape the 🆔 lines). Surface it as data so tools chain.
4. **Read/download attachments** — walk payload.parts + `attachments.get`. HIGH / MED. `read-email` already fetches format:full. **Ties into the new local storage** (save an attachment → hermes-storage).
5. Full-thread view (`threads.get`), Drafts (`drafts.*`), Send attachments (multipart MIME), Label CRUD — MED, in that order.

**Latent bugs to fix while in the send path (near-free, worth doing):**
- `reply-email` sets In-Reply-To/References to Gmail's **internal message id**, not the RFC822 `Message-ID` header → threading breaks in non-Gmail clients.
- **No RFC 2047 subject encoding** → a `Re: São Paulo` subject produces a broken header (this is a BR operator — non-ASCII subjects are the norm).
- `read-email` HTML→text is naive (`replace(/<[^>]*>/g,'')`) — leaves entities, doesn't strip style/script contents.
- **No pagination anywhere** (nextPageToken never read) → list/search silently truncate (get-emails month caps at 100, drops older mail with no flag).
- **N+1 unbatched** — search fans up to 500 concurrent `messages.get` → rate-limit risk. Only trash throttles.
- `trash-emails` partial failure silently drops the failed IDs from the count (reports successes only).

## Calendar (4 tools) — scope `calendar` (full)

**Ranked adds (all supported by Calendar API v3):**
1. **Update / reschedule / move event** — `events.patch` (already used by completeEvent). HIGH / LOW. Single most common assistant edit; today you can only create + recolor. **Top pick.**
2. **Delete / cancel event** — `events.delete` + sendUpdates. HIGH / LOW. No way to remove a mistaken event today.
3. **Find-free-slots / availability** — `freebusy.query` (or generalize the slot-walk already in task.use-case). HIGH / MED.
4. **Attendee/RSVP management + expose `sendUpdates`** — LOW-MED. Also fixes the papercut that create **forces `sendUpdates:'all'`** (every create emails all attendees, no opt-out).
5. Reminders control (create **force-disables all reminders** — you can't make an event that reminds you; expose overrides), multi-calendar (everything hard-pinned to `primary`), single-instance recurring edits — MED.

**Papercuts:** forced attendee emails; force-disabled reminders; `location` accepted but never echoed; all-day create path `moment(input.start)` без tz can slip a day; batch create is Promise.all all-or-nothing; `get-today-agenda` parses `new Date("YYYY-MM-DD")` as UTC (day-boundary bug) and is redundant with `get-agenda preset:today`.

## Contacts (5 tools) — scope full `contacts` (writes work)
- Writes are authorized (correction above). Improvements: **multi-value fields** (create/update flatten to a single email/phone/org; update REPLACES the list rather than merging) — MED/LOW; contact **groups/labels** (`contactGroups.*`) — MED; batch ops — LOW-MED. Skip merge/dedupe (no API, high effort).
- Papercuts: `createContact` puts the full name in `givenName` (no family split); `search` covers name+email+phone but not org/notes (Google limitation); People `searchContacts` warm-up/stale-cache caveat unhandled.
- **FIX THE MINT SCRIPT** (readonly drift, above) so a future re-mint doesn't silently break writes.

## Tasks (3 tools) — DECISION: the calendar-hack is actively harmful

The hack pollutes the primary calendar with 1–5 AM phantom events, **collides with any real red/pink event** (color is the only "task" signal), caps at ~4–16 tasks/day, and **forgets any task not completed today** (`get-tasks` reads only today). That's worse than no task tool.

**⚠️ The round-3 pivot changed the economics.** Round-2 said "fix Tasks by folding the tasks scope into the Drive re-consent (near-free)." But **the operator DROPPED Drive in round 3 specifically to avoid re-consent friction** — so there is NO re-consent on the table anymore. A real-Tasks-API rewrite now needs its OWN standalone re-consent (add `auth/tasks`), which is exactly the friction the operator rejected. So the options are:
- **(a) REMOVE the calendar hack** — cleanest immediate win, stops the calendar pollution, no re-consent. Recommended if the operator doesn't want a standalone Google re-consent.
- **(b) REBUILD tasks on the NEW local-storage layer** — a local task list (title/due/notes/done) backed by the hermes-storage work already being built, NO Google scope needed. Interesting middle path — keeps a task concept without re-consent OR calendar pollution.
- **(c) FIX to the real Google Tasks API** — best-quality (lists/due-dates/subtasks/real completed state) but needs the standalone re-consent the operator has been avoiding.
- **Do NOT leave the calendar hack as-is** under any choice.

**Recommendation:** (a) or (b). Given the operator's demonstrated re-consent aversion, either remove it or fold a lightweight task list into the local-storage build. (c) only if the operator will do a tasks-only re-consent.

**Also delete `complete-calendar-event` / `complete-calendar-events`:** mis-modeled — events aren't "completable"; it just overwrites colorId to graphite (lossy, meaningless to Google, identical to complete-task, applies to any event incl. real meetings). It only exists to prop up the tasks hack; remove it when tasks move off the hack.

## Cross-cutting (all Google tools)
- No pagination anywhere (nextPageToken never read) → silent truncation.
- N+1 unbatched list-then-get; Promise.all all-or-nothing batches (one failure loses the batch, no per-item status).
- Timezone weak spots: `get-today-agenda` UTC parse + all-day create no-tz — both slip a day at boundaries.
- **Strength worth preserving:** the error-handling infra (withRetry/withLogging, typed errors, forced errorContext per tool) is genuinely good — build on it, don't replace it. (Caveat: on StarkBank that same retry infra is the R3 booby-trap — money surface needs the opposite treatment.)

---

## Operator decision items
1. **StarkBank:** approve a Cipher security review as a gate before any banking-tool feature work (escalated separately). Highest priority.
2. **Gmail:** approve the top-3 (send-new + modify-state + structured output) — near-zero effort, no re-consent, closes the biggest everyday gaps.
3. **Calendar:** approve update/delete/free-slots — low effort, high value.
4. **Tasks:** pick (a) remove the hack / (b) rebuild on local storage / (c) real Tasks API (needs a tasks-only re-consent). Recommend (a) or (b) given re-consent aversion.
5. **Contacts:** fix the mint-script readonly drift (latent trap); optional multi-value-field improvement.

## Recommended order (non-StarkBank)
Gmail top-3 (biggest value, zero friction) → Calendar update/delete/free-slots → Contacts mint-script fix → Tasks decision (a/b). StarkBank waits on Cipher.
