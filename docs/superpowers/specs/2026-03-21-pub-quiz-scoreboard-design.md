# Pub Quiz Scoreboard — Design Spec

**Date:** 2026-03-21
**Status:** Approved

---

## Overview

A simple web app to run a pub quiz night. The host creates a session, adds teams, records scores round by round, and a leaderboard shows cumulative standings. No auth, no real-time push — page refreshes on submit are sufficient.

---

## Stack

- **Backend:** Node.js (Express)
- **Database:** PostgreSQL (IDs are `SERIAL` integers throughout)
- **Frontend:** Plain HTML/CSS/JS (no framework)

---

## Data Model

```sql
sessions (
  id          SERIAL PRIMARY KEY,
  name        TEXT NOT NULL,
  created_at  TIMESTAMP DEFAULT NOW(),
  finished_at TIMESTAMP   -- NULL means active
)

teams (
  id          SERIAL PRIMARY KEY,
  session_id  INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  UNIQUE(session_id, name)
)

rounds (
  id          SERIAL PRIMARY KEY,
  session_id  INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  number      INTEGER NOT NULL,   -- display order (1, 2, 3...), per-session
  label       TEXT,               -- e.g. "Round 1 - Geography" (optional)
  UNIQUE(session_id, number)
)

scores (
  id          SERIAL PRIMARY KEY,
  team_id     INTEGER NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  round_id    INTEGER NOT NULL REFERENCES rounds(id) ON DELETE CASCADE,
  points      INTEGER NOT NULL DEFAULT 0 CHECK (points >= 0),
  UNIQUE(team_id, round_id)
)
```

Rounds are ordered by `number ASC` in all queries. Round `number` is scoped per session (session A and session B can both have a round #1). The `UNIQUE(session_id, number)` constraint prevents duplicate round numbers per session. `number` is assigned as `SELECT COALESCE(MAX(number), 0) + 1 FROM rounds WHERE session_id = $1` inside a transaction to avoid race conditions.

---

## API Endpoints

### Sessions

**GET /sessions**
Returns all sessions (including finished), ordered by `created_at DESC`.
```json
[{ "id": 1, "name": "Friday Night Trivia", "created_at": "...", "finished_at": null }]
```

**POST /sessions**
Body: `{ "name": "string" }`
Returns: created session object. 400 if name is missing or empty.

**GET /sessions/:id**
Returns session with nested teams, rounds, and scores. Scores are sparse — only entries that exist appear; a team with no score for a round is omitted from that round's `scores` array. Frontend uses the teams list to render all input rows and falls back to 0 for missing entries.
```json
{
  "id": 1,
  "name": "Friday Night Trivia",
  "finished_at": null,
  "teams": [{ "id": 1, "name": "Team A" }],
  "rounds": [
    {
      "id": 1,
      "number": 1,
      "label": "Geography",
      "scores": [{ "team_id": 1, "points": 8 }]
    }
  ]
}
```
Returns 404 if session not found.

**PATCH /sessions/:id/finish**
Marks session finished by setting `finished_at = NOW()`. Idempotent — if already finished, returns the current session object unchanged. Returns 404 if session not found. Returns the updated session object on success.

### Teams

**POST /sessions/:id/teams**
Body: `{ "name": "string" }`
Returns created team. 400 if name empty. 404 if session not found. 409 if team name already exists in session (enforced by DB unique constraint). 403 if session is finished.

### Rounds

**POST /sessions/:id/rounds**
Body: `{ "label": "string" }` (label is optional)
Assigns `number` as `MAX(number) + 1` for the session inside a transaction; `UNIQUE(session_id, number)` prevents duplicates under concurrency.
Returns created round. 404 if session not found. 403 if session is finished.

### Scores

**POST /rounds/:id/scores**
Body: `{ "scores": [{ "team_id": 1, "points": 8 }, ...] }`
Partial submissions are allowed — only the team_ids in the body are upserted. Omitted teams retain their existing scores.
Upserts each entry: `INSERT ... ON CONFLICT (team_id, round_id) DO UPDATE SET points = EXCLUDED.points`.
All `team_id` values must belong to the same session as the round (validated in app before upsert). Returns 400 if any `team_id` is invalid or points < 0. Returns 403 if session is finished. Returns 404 if round not found.
Score inputs in the UI are pre-populated with existing scores on page load so the host sees current values before editing.
Returns: `{ "round_id": 1, "scores": [{ "team_id": 1, "points": 8 }] }`

### Leaderboard

**GET /sessions/:id/leaderboard**
Returns teams sorted by total points descending. Ties broken alphabetically by team name. Ranking uses `DENSE_RANK()` (no gaps on ties).
```json
[
  { "rank": 1, "team_id": 2, "name": "Team B", "total": 42 },
  { "rank": 2, "team_id": 1, "name": "Team A", "total": 35 }
]
```
Teams with no scores appear at the bottom with `total: 0`. Returns 404 if session not found.

---

## Pages

### `/` — Home
- List of all sessions (name, status, created date)
- "New Session" form (name input + submit)

### `/session/:id` — Session View
- Session name + "Finished" badge if `finished_at` is set
- Team list with current totals
- "Add Team" form (disabled if finished)
- Round list with per-team score inputs pre-populated from existing scores; "Submit Scores" button per round (disabled if finished)
- Leaderboard table (rank, team name, total) — re-fetched from `/sessions/:id/leaderboard` after each score submission
- "End Quiz" button — calls `PATCH /sessions/:id/finish`, then re-renders page in finished state (disabled if already finished)

---

## Error Handling

| HTTP Code | Meaning |
|-----------|---------|
| 400 | Bad request (missing/invalid fields, points < 0, invalid team_id in scores) |
| 403 | Action not allowed (modifying a finished session, team not in session) |
| 404 | Resource not found |
| 409 | Conflict (duplicate team name within session) |
| 500 | Unexpected server error |

All errors return: `{ "error": "human-readable message" }`
Frontend shows inline error messages below the relevant form.

---

## Session Finish Behaviour

Once a session is marked finished:
- No new teams or rounds can be added (403)
- No scores can be submitted (403)
- The UI disables all input forms and shows a "Finished" badge
- "End Quiz" button is disabled
- Leaderboard remains visible and readable

---

## Acceptance Criteria

- [ ] Can create a named session and see it on the home page
- [ ] Can add multiple teams to a session; duplicate team name in same session returns 409
- [ ] Can add rounds (with optional label) and enter scores per team
- [ ] Score inputs are pre-populated with existing values on page load
- [ ] Leaderboard shows correct cumulative totals, sorted descending; ties broken alphabetically; uses dense ranking
- [ ] Teams with no scores show 0 in leaderboard
- [ ] Submitting scores twice for the same team/round upserts (updates) the score
- [ ] Clicking "End Quiz" marks session finished; page re-renders with all inputs disabled and "Finished" badge shown
- [ ] Modifying a finished session (add team, add round, submit scores) returns 403
- [ ] 404 is returned for unknown session/team/round IDs
- [ ] App starts successfully when `DATABASE_URL` env var is set; logs a clear error and exits if missing

---

## Environment & Deployment

**Environment variables:**
- `DATABASE_URL` — PostgreSQL connection string (e.g. `postgres://user:pass@host:5432/pubquiz`)
- `PORT` — HTTP port, defaults to 3000

**Schema:** Applied via `migrations/001_init.sql`, run once on first deploy:
```bash
psql $DATABASE_URL < migrations/001_init.sql
```

**Deploy target:** Node.js service on port 3000, PostgreSQL database, deployed to Dokploy.
