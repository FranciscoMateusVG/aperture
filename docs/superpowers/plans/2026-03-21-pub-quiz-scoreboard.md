# Pub Quiz Scoreboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Node.js + PostgreSQL web app where a quiz host creates sessions, adds teams, records round scores, and sees a live leaderboard.

**Architecture:** Express API serving JSON to a plain HTML/JS frontend. PostgreSQL stores all data. Frontend fetches API on each action and re-renders relevant sections. No build step — the `public/` folder is served statically.

**Tech Stack:** Node.js 20+, Express 4, pg (node-postgres), dotenv, Jest + Supertest (tests)

> **Testing note:** Tests use a separate `TEST_DATABASE_URL` pointing at a `pubquiz_test` database, selected via `NODE_ENV=test`. This is not a spec requirement — it's a standard safeguard so tests never touch production data. You must create this test database manually (`createdb pubquiz_test`) and apply the migration to it. The `.env.example` documents both variables.

**Spec:** `docs/superpowers/specs/2026-03-21-pub-quiz-scoreboard-design.md`

**Project root:** `/Users/<your-username>/projects/pub-quiz/`

---

## File Structure

```
pub-quiz/
├── package.json
├── .env                    # local secrets (git-ignored)
├── .env.example            # template committed to git
├── .gitignore
├── migrations/
│   └── 001_init.sql        # full schema DDL — run once on first deploy
├── src/
│   ├── index.js            # entry point — starts HTTP server, validates env
│   ├── app.js              # Express app — mounts routes, static files, error handler
│   ├── db.js               # pg Pool singleton — exported and used by all routes
│   └── routes/
│       ├── sessions.js     # GET/POST /sessions, GET/PATCH /sessions/:id
│       ├── teams.js        # POST /sessions/:id/teams
│       ├── rounds.js       # POST /sessions/:id/rounds
│       ├── scores.js       # POST /rounds/:id/scores
│       └── leaderboard.js  # GET /sessions/:id/leaderboard
├── public/
│   ├── index.html          # home page: list sessions, create session
│   ├── session.html        # session view: teams, rounds, scores, leaderboard
│   ├── style.css           # shared styles
│   ├── home.js             # JS for home page
│   └── session.js          # JS for session page
└── tests/
    ├── setup.js             # test DB helpers — truncate before each suite
    ├── sessions.test.js
    ├── teams.test.js
    ├── rounds.test.js
    ├── scores.test.js
    └── leaderboard.test.js
```

---

## Task 1: Project Setup

**Files:**
- Create: `pub-quiz/package.json`
- Create: `pub-quiz/.env.example`
- Create: `pub-quiz/.gitignore`

- [ ] **Step 1: Create project directory and init npm**

```bash
mkdir -p /Users/<your-username>/projects/pub-quiz
cd /Users/<your-username>/projects/pub-quiz
npm init -y
```

- [ ] **Step 2: Install dependencies**

```bash
npm install express pg dotenv
npm install --save-dev jest supertest
```

- [ ] **Step 3: Update package.json with scripts**

Edit `package.json` so the `scripts` block looks like:

```json
"scripts": {
  "start": "node src/index.js",
  "dev": "node --watch src/index.js",
  "test": "jest --runInBand"
}
```

Add jest config at the bottom of `package.json`:

```json
"jest": {
  "testEnvironment": "node",
  "testTimeout": 10000
}
```

- [ ] **Step 4: Create .env.example**

```
DATABASE_URL=postgres://user:password@localhost:5432/pubquiz
TEST_DATABASE_URL=postgres://user:password@localhost:5432/pubquiz_test
PORT=3000
```

- [ ] **Step 5: Create .gitignore**

```
node_modules/
.env
```

- [ ] **Step 6: Create .env from example and fill in real values**

```bash
cp .env.example .env
# edit .env with your actual Postgres connection strings
# make sure the pubquiz and pubquiz_test databases exist:
# createdb pubquiz
# createdb pubquiz_test
```

- [ ] **Step 7: Init git and commit**

```bash
git init
git add package.json package-lock.json .env.example .gitignore
git commit -m "chore: project setup"
```

---

## Task 2: Database Migration

**Files:**
- Create: `migrations/001_init.sql`
- Create: `src/db.js`

- [ ] **Step 1: Write the migration file**

Create `migrations/001_init.sql`:

```sql
CREATE TABLE IF NOT EXISTS sessions (
  id          SERIAL PRIMARY KEY,
  name        TEXT NOT NULL,
  created_at  TIMESTAMP DEFAULT NOW(),
  finished_at TIMESTAMP
);

CREATE TABLE IF NOT EXISTS teams (
  id          SERIAL PRIMARY KEY,
  session_id  INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  UNIQUE(session_id, name)
);

CREATE TABLE IF NOT EXISTS rounds (
  id          SERIAL PRIMARY KEY,
  session_id  INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  number      INTEGER NOT NULL,
  label       TEXT,
  UNIQUE(session_id, number)
);

CREATE TABLE IF NOT EXISTS scores (
  id          SERIAL PRIMARY KEY,
  team_id     INTEGER NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  round_id    INTEGER NOT NULL REFERENCES rounds(id) ON DELETE CASCADE,
  points      INTEGER NOT NULL DEFAULT 0 CHECK (points >= 0),
  UNIQUE(team_id, round_id)
);
```

- [ ] **Step 2: Apply migration to both databases**

```bash
psql $DATABASE_URL < migrations/001_init.sql
psql $TEST_DATABASE_URL < migrations/001_init.sql
```

Expected: each command prints a series of `CREATE TABLE` lines with no errors.

- [ ] **Step 3: Write src/db.js**

```js
const { Pool } = require('pg')
require('dotenv').config()

const connectionString = process.env.NODE_ENV === 'test'
  ? process.env.TEST_DATABASE_URL
  : process.env.DATABASE_URL

if (!connectionString) {
  console.error('ERROR: DATABASE_URL (or TEST_DATABASE_URL in test mode) is not set.')
  process.exit(1)
}

const pool = new Pool({ connectionString })

module.exports = pool
```

- [ ] **Step 4: Commit**

```bash
git add migrations/001_init.sql src/db.js
git commit -m "feat: database schema and pg pool"
```

---

## Task 3: Express App Skeleton

**Files:**
- Create: `src/app.js`
- Create: `src/index.js`

- [ ] **Step 1: Write src/app.js**

```js
const express = require('express')
const path = require('path')

const sessionsRouter = require('./routes/sessions')
const teamsRouter = require('./routes/teams')
const roundsRouter = require('./routes/rounds')
const scoresRouter = require('./routes/scores')
const leaderboardRouter = require('./routes/leaderboard')

const app = express()

app.use(express.json())
app.use(express.static(path.join(__dirname, '..', 'public')))

app.use('/sessions', sessionsRouter)
app.use('/sessions', teamsRouter)
app.use('/sessions', roundsRouter)
app.use('/rounds', scoresRouter)
app.use('/sessions', leaderboardRouter)

// 404 handler
app.use((req, res) => {
  res.status(404).json({ error: 'Not found' })
})

// Error handler — all errors MUST use { "error": "message" } shape per spec
// Routes can pass a custom status via err.status (e.g. Object.assign(new Error('msg'), { status: 409 }))
app.use((err, req, res, next) => {
  console.error(err)
  const status = err.status || 500
  res.status(status).json({ error: err.message || 'Internal server error' })
})

module.exports = app
```

- [ ] **Step 2: Write src/index.js**

```js
require('dotenv').config()

const app = require('./app')

const PORT = process.env.PORT || 3000

app.listen(PORT, () => {
  console.log(`Pub Quiz running on http://localhost:${PORT}`)
})
```

- [ ] **Step 3: Create route stubs so the app can start**

Create `src/routes/sessions.js`, `src/routes/teams.js`, `src/routes/rounds.js`, `src/routes/scores.js`, `src/routes/leaderboard.js` — each with the same stub:

```js
const { Router } = require('express')
const router = Router()
module.exports = router
```

- [ ] **Step 4: Verify app starts**

```bash
node src/index.js
```

Expected: `Pub Quiz running on http://localhost:3000`
Kill with Ctrl+C.

- [ ] **Step 5: Commit**

```bash
git add src/app.js src/index.js src/routes/
git commit -m "feat: express app skeleton with route stubs"
```

---

## Task 4: Sessions API

**Files:**
- Modify: `src/routes/sessions.js`
- Create: `tests/setup.js`
- Create: `tests/sessions.test.js`

- [ ] **Step 1: Write tests/setup.js**

```js
const pool = require('../src/db')

async function truncateAll() {
  await pool.query('TRUNCATE sessions CASCADE')
}

async function closePool() {
  await pool.end()
}

module.exports = { truncateAll, closePool }
```

- [ ] **Step 2: Write the failing tests**

Create `tests/sessions.test.js`:

```js
process.env.NODE_ENV = 'test'

const request = require('supertest')
const app = require('../src/app')
const { truncateAll, closePool } = require('./setup')

beforeEach(truncateAll)
afterAll(closePool)

describe('POST /sessions', () => {
  it('creates a session and returns it', async () => {
    const res = await request(app).post('/sessions').send({ name: 'Friday Night' })
    expect(res.status).toBe(201)
    expect(res.body.name).toBe('Friday Night')
    expect(res.body.id).toBeDefined()
    expect(res.body.finished_at).toBeNull()
  })

  it('returns 400 if name is missing', async () => {
    const res = await request(app).post('/sessions').send({})
    expect(res.status).toBe(400)
    expect(res.body.error).toBeDefined()
  })
})

describe('GET /sessions', () => {
  it('returns all sessions ordered by created_at DESC', async () => {
    await request(app).post('/sessions').send({ name: 'First' })
    await request(app).post('/sessions').send({ name: 'Second' })
    const res = await request(app).get('/sessions')
    expect(res.status).toBe(200)
    expect(res.body[0].name).toBe('Second')
    expect(res.body[1].name).toBe('First')
  })
})

describe('GET /sessions/:id', () => {
  it('returns 404 for unknown session', async () => {
    const res = await request(app).get('/sessions/99999')
    expect(res.status).toBe(404)
  })

  it('returns session with empty teams and rounds', async () => {
    const { body: session } = await request(app).post('/sessions').send({ name: 'Test' })
    const res = await request(app).get(`/sessions/${session.id}`)
    expect(res.status).toBe(200)
    expect(res.body.teams).toEqual([])
    expect(res.body.rounds).toEqual([])
  })
})

describe('PATCH /sessions/:id/finish', () => {
  it('marks a session as finished', async () => {
    const { body: session } = await request(app).post('/sessions').send({ name: 'Test' })
    const res = await request(app).patch(`/sessions/${session.id}/finish`)
    expect(res.status).toBe(200)
    expect(res.body.finished_at).not.toBeNull()
  })

  it('is idempotent — returns 200 if already finished', async () => {
    const { body: session } = await request(app).post('/sessions').send({ name: 'Test' })
    await request(app).patch(`/sessions/${session.id}/finish`)
    const res = await request(app).patch(`/sessions/${session.id}/finish`)
    expect(res.status).toBe(200)
  })

  it('returns 404 for unknown session', async () => {
    const res = await request(app).patch('/sessions/99999/finish')
    expect(res.status).toBe(404)
  })
})
```

- [ ] **Step 3: Run tests — verify they all fail**

```bash
NODE_ENV=test npx jest tests/sessions.test.js --verbose
```

Expected: all tests FAIL (routes are stubs).

- [ ] **Step 4: Implement sessions routes**

Replace `src/routes/sessions.js` with:

```js
const { Router } = require('express')
const pool = require('../db')
const router = Router()

// GET /sessions
router.get('/', async (req, res, next) => {
  try {
    const { rows } = await pool.query(
      'SELECT * FROM sessions ORDER BY created_at DESC'
    )
    res.json(rows)
  } catch (err) { next(err) }
})

// POST /sessions
router.post('/', async (req, res, next) => {
  try {
    const { name } = req.body
    if (!name || !name.trim()) {
      return res.status(400).json({ error: 'name is required' })
    }
    const { rows } = await pool.query(
      'INSERT INTO sessions (name) VALUES ($1) RETURNING *',
      [name.trim()]
    )
    res.status(201).json(rows[0])
  } catch (err) { next(err) }
})

// GET /sessions/:id
router.get('/:id', async (req, res, next) => {
  try {
    const { id } = req.params
    const { rows: sessions } = await pool.query(
      'SELECT * FROM sessions WHERE id = $1', [id]
    )
    if (!sessions.length) return res.status(404).json({ error: 'Session not found' })

    const session = sessions[0]

    const { rows: teams } = await pool.query(
      'SELECT id, name FROM teams WHERE session_id = $1 ORDER BY id', [id]
    )

    const { rows: roundRows } = await pool.query(
      'SELECT * FROM rounds WHERE session_id = $1 ORDER BY number', [id]
    )

    const { rows: scoreRows } = await pool.query(
      `SELECT s.team_id, s.round_id, s.points
       FROM scores s
       JOIN rounds r ON s.round_id = r.id
       WHERE r.session_id = $1`, [id]
    )

    const rounds = roundRows.map(r => ({
      ...r,
      scores: scoreRows
        .filter(s => s.round_id === r.id)
        .map(s => ({ team_id: s.team_id, points: s.points }))
    }))

    res.json({ ...session, teams, rounds })
  } catch (err) { next(err) }
})

// PATCH /sessions/:id/finish
router.patch('/:id/finish', async (req, res, next) => {
  try {
    const { id } = req.params
    // Single UPDATE with RETURNING — avoids TOCTOU race of SELECT then UPDATE.
    // COALESCE keeps existing finished_at if already set (idempotent).
    // rowCount === 0 means session didn't exist.
    const { rows, rowCount } = await pool.query(
      `UPDATE sessions
       SET finished_at = COALESCE(finished_at, NOW())
       WHERE id = $1
       RETURNING *`,
      [id]
    )
    if (rowCount === 0) return res.status(404).json({ error: 'Session not found' })
    res.json(rows[0])
  } catch (err) { next(err) }
})

module.exports = router
```

- [ ] **Step 5: Run tests — verify they all pass**

```bash
NODE_ENV=test npx jest tests/sessions.test.js --verbose
```

Expected: all tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/routes/sessions.js tests/setup.js tests/sessions.test.js
git commit -m "feat: sessions API with tests"
```

---

## Task 5: Teams API

**Files:**
- Modify: `src/routes/teams.js`
- Create: `tests/teams.test.js`

- [ ] **Step 1: Write the failing tests**

Create `tests/teams.test.js`:

```js
process.env.NODE_ENV = 'test'

const request = require('supertest')
const app = require('../src/app')
const { truncateAll, closePool } = require('./setup')

beforeEach(truncateAll)
afterAll(closePool)

async function createSession(name = 'Test Session') {
  const res = await request(app).post('/sessions').send({ name })
  return res.body
}

describe('POST /sessions/:id/teams', () => {
  it('adds a team to a session', async () => {
    const session = await createSession()
    const res = await request(app)
      .post(`/sessions/${session.id}/teams`)
      .send({ name: 'Team A' })
    expect(res.status).toBe(201)
    expect(res.body.name).toBe('Team A')
    expect(res.body.session_id).toBe(session.id)
  })

  it('returns 400 if name is missing', async () => {
    const session = await createSession()
    const res = await request(app)
      .post(`/sessions/${session.id}/teams`)
      .send({})
    expect(res.status).toBe(400)
  })

  it('returns 404 for unknown session', async () => {
    const res = await request(app)
      .post('/sessions/99999/teams')
      .send({ name: 'Team A' })
    expect(res.status).toBe(404)
  })

  it('returns 409 for duplicate team name in same session', async () => {
    const session = await createSession()
    await request(app).post(`/sessions/${session.id}/teams`).send({ name: 'Team A' })
    const res = await request(app)
      .post(`/sessions/${session.id}/teams`)
      .send({ name: 'Team A' })
    expect(res.status).toBe(409)
  })

  it('returns 403 if session is finished', async () => {
    const session = await createSession()
    await request(app).patch(`/sessions/${session.id}/finish`)
    const res = await request(app)
      .post(`/sessions/${session.id}/teams`)
      .send({ name: 'Late Team' })
    expect(res.status).toBe(403)
  })
})
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
NODE_ENV=test npx jest tests/teams.test.js --verbose
```

Expected: FAIL.

- [ ] **Step 3: Implement teams route**

Replace `src/routes/teams.js`:

```js
const { Router } = require('express')
const pool = require('../db')
const router = Router()

// POST /sessions/:id/teams
router.post('/:id/teams', async (req, res, next) => {
  try {
    const { id } = req.params
    const { name } = req.body

    if (!name || !name.trim()) {
      return res.status(400).json({ error: 'name is required' })
    }

    const { rows: sessions } = await pool.query(
      'SELECT * FROM sessions WHERE id = $1', [id]
    )
    if (!sessions.length) return res.status(404).json({ error: 'Session not found' })
    if (sessions[0].finished_at) return res.status(403).json({ error: 'Session is finished' })

    try {
      const { rows } = await pool.query(
        'INSERT INTO teams (session_id, name) VALUES ($1, $2) RETURNING *',
        [id, name.trim()]
      )
      res.status(201).json(rows[0])
    } catch (err) {
      if (err.code === '23505') { // unique violation
        return res.status(409).json({ error: 'Team name already exists in this session' })
      }
      throw err
    }
  } catch (err) { next(err) }
})

module.exports = router
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
NODE_ENV=test npx jest tests/teams.test.js --verbose
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/routes/teams.js tests/teams.test.js
git commit -m "feat: teams API with tests"
```

---

## Task 6: Rounds API

**Files:**
- Modify: `src/routes/rounds.js`
- Create: `tests/rounds.test.js`

- [ ] **Step 1: Write the failing tests**

Create `tests/rounds.test.js`:

```js
process.env.NODE_ENV = 'test'

const request = require('supertest')
const app = require('../src/app')
const { truncateAll, closePool } = require('./setup')

beforeEach(truncateAll)
afterAll(closePool)

async function createSession() {
  const res = await request(app).post('/sessions').send({ name: 'Test' })
  return res.body
}

describe('POST /sessions/:id/rounds', () => {
  it('creates a round with auto-incremented number', async () => {
    const session = await createSession()
    const res = await request(app)
      .post(`/sessions/${session.id}/rounds`)
      .send({ label: 'Geography' })
    expect(res.status).toBe(201)
    expect(res.body.number).toBe(1)
    expect(res.body.label).toBe('Geography')
  })

  it('increments number for each new round', async () => {
    const session = await createSession()
    await request(app).post(`/sessions/${session.id}/rounds`).send({})
    const res = await request(app).post(`/sessions/${session.id}/rounds`).send({})
    expect(res.body.number).toBe(2)
  })

  it('label is optional — defaults to null', async () => {
    const session = await createSession()
    const res = await request(app).post(`/sessions/${session.id}/rounds`).send({})
    expect(res.status).toBe(201)
    expect(res.body.label).toBeNull()
  })

  it('returns 404 for unknown session', async () => {
    const res = await request(app).post('/sessions/99999/rounds').send({})
    expect(res.status).toBe(404)
  })

  it('returns 403 if session is finished', async () => {
    const session = await createSession()
    await request(app).patch(`/sessions/${session.id}/finish`)
    const res = await request(app).post(`/sessions/${session.id}/rounds`).send({})
    expect(res.status).toBe(403)
  })
})
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
NODE_ENV=test npx jest tests/rounds.test.js --verbose
```

- [ ] **Step 3: Implement rounds route**

Replace `src/routes/rounds.js`:

```js
const { Router } = require('express')
const pool = require('../db')
const router = Router()

// POST /sessions/:id/rounds
router.post('/:id/rounds', async (req, res, next) => {
  const client = await pool.connect()
  try {
    const { id } = req.params
    const label = req.body.label || null

    await client.query('BEGIN')

    const { rows: sessions } = await client.query(
      'SELECT * FROM sessions WHERE id = $1', [id]
    )
    if (!sessions.length) {
      await client.query('ROLLBACK')
      return res.status(404).json({ error: 'Session not found' })
    }
    if (sessions[0].finished_at) {
      await client.query('ROLLBACK')
      return res.status(403).json({ error: 'Session is finished' })
    }

    const { rows: maxRows } = await client.query(
      // COALESCE handles first round (MAX returns NULL when no rows exist)
      'SELECT COALESCE(MAX(number), 0) + 1 AS next_num FROM rounds WHERE session_id = $1',
      [id]
    )
    const nextNum = maxRows[0].next_num

    const { rows } = await client.query(
      'INSERT INTO rounds (session_id, number, label) VALUES ($1, $2, $3) RETURNING *',
      [id, nextNum, label]
    )

    await client.query('COMMIT')
    res.status(201).json(rows[0])
  } catch (err) {
    await client.query('ROLLBACK')
    // UNIQUE(session_id, number) violated under concurrency — return 409
    if (err.code === '23505') {
      return next(Object.assign(new Error('Round number conflict, please try again'), { status: 409 }))
    }
    next(err)
  } finally {
    client.release()
  }
})

module.exports = router
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
NODE_ENV=test npx jest tests/rounds.test.js --verbose
```

- [ ] **Step 5: Commit**

```bash
git add src/routes/rounds.js tests/rounds.test.js
git commit -m "feat: rounds API with tests"
```

---

## Task 7: Scores API

**Files:**
- Modify: `src/routes/scores.js`
- Create: `tests/scores.test.js`

- [ ] **Step 1: Write the failing tests**

Create `tests/scores.test.js`:

```js
process.env.NODE_ENV = 'test'

const request = require('supertest')
const app = require('../src/app')
const { truncateAll, closePool } = require('./setup')

beforeEach(truncateAll)
afterAll(closePool)

async function setup() {
  const { body: session } = await request(app).post('/sessions').send({ name: 'Quiz' })
  const { body: teamA } = await request(app).post(`/sessions/${session.id}/teams`).send({ name: 'A' })
  const { body: teamB } = await request(app).post(`/sessions/${session.id}/teams`).send({ name: 'B' })
  const { body: round } = await request(app).post(`/sessions/${session.id}/rounds`).send({ label: 'R1' })
  return { session, teamA, teamB, round }
}

describe('POST /rounds/:id/scores', () => {
  it('upserts scores for given teams', async () => {
    const { teamA, round } = await setup()
    const res = await request(app)
      .post(`/rounds/${round.id}/scores`)
      .send({ scores: [{ team_id: teamA.id, points: 10 }] })
    expect(res.status).toBe(200)
    expect(res.body.scores[0].points).toBe(10)
  })

  it('upserts (updates) an existing score', async () => {
    const { teamA, round } = await setup()
    await request(app).post(`/rounds/${round.id}/scores`).send({ scores: [{ team_id: teamA.id, points: 5 }] })
    const res = await request(app).post(`/rounds/${round.id}/scores`).send({ scores: [{ team_id: teamA.id, points: 9 }] })
    expect(res.status).toBe(200)
    expect(res.body.scores[0].points).toBe(9)
  })

  it('allows partial submission — omitted teams keep existing scores', async () => {
    const { teamA, teamB, round, session } = await setup()
    await request(app).post(`/rounds/${round.id}/scores`).send({
      scores: [{ team_id: teamA.id, points: 5 }, { team_id: teamB.id, points: 3 }]
    })
    await request(app).post(`/rounds/${round.id}/scores`).send({
      scores: [{ team_id: teamA.id, points: 9 }]  // only update A
    })
    const detail = await request(app).get(`/sessions/${session.id}`)
    const r = detail.body.rounds[0]
    const bScore = r.scores.find(s => s.team_id === teamB.id)
    expect(bScore.points).toBe(3)  // B unchanged
  })

  it('returns 400 if points < 0', async () => {
    const { teamA, round } = await setup()
    const res = await request(app)
      .post(`/rounds/${round.id}/scores`)
      .send({ scores: [{ team_id: teamA.id, points: -1 }] })
    expect(res.status).toBe(400)
  })

  it('returns 400 if team_id does not belong to session', async () => {
    const { round } = await setup()
    const res = await request(app)
      .post(`/rounds/${round.id}/scores`)
      .send({ scores: [{ team_id: 99999, points: 5 }] })
    expect(res.status).toBe(400)
  })

  it('returns 404 for unknown round', async () => {
    const res = await request(app)
      .post('/rounds/99999/scores')
      .send({ scores: [] })
    expect(res.status).toBe(404)
  })

  it('returns 403 if session is finished', async () => {
    const { session, teamA, round } = await setup()
    await request(app).patch(`/sessions/${session.id}/finish`)
    const res = await request(app)
      .post(`/rounds/${round.id}/scores`)
      .send({ scores: [{ team_id: teamA.id, points: 5 }] })
    expect(res.status).toBe(403)
  })
})
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
NODE_ENV=test npx jest tests/scores.test.js --verbose
```

- [ ] **Step 3: Implement scores route**

Replace `src/routes/scores.js`:

```js
const { Router } = require('express')
const pool = require('../db')
const router = Router()

// POST /rounds/:id/scores
router.post('/:id/scores', async (req, res, next) => {
  try {
    const { id: roundId } = req.params
    const { scores } = req.body

    if (!Array.isArray(scores)) {
      return res.status(400).json({ error: 'scores must be an array' })
    }

    // validate points >= 0
    for (const s of scores) {
      if (typeof s.points !== 'number' || s.points < 0) {
        return res.status(400).json({ error: 'points must be a non-negative number' })
      }
    }

    // fetch round + session
    const { rows: rounds } = await pool.query(
      `SELECT r.*, s.finished_at
       FROM rounds r JOIN sessions s ON s.id = r.session_id
       WHERE r.id = $1`, [roundId]
    )
    if (!rounds.length) return res.status(404).json({ error: 'Round not found' })
    if (rounds[0].finished_at) return res.status(403).json({ error: 'Session is finished' })

    const sessionId = rounds[0].session_id

    // validate team membership — team_id not in this session → 400 (invalid field value)
    // Note: spec error table lists 403 for "team not in session" but also 400 for "invalid team_id
    // in scores". We use 400 here as the team_id is simply invalid input, not an auth issue.
    if (scores.length > 0) {
      const teamIds = scores.map(s => s.team_id)
      const { rows: validTeams } = await pool.query(
        'SELECT id FROM teams WHERE session_id = $1 AND id = ANY($2::int[])',
        [sessionId, teamIds]
      )
      const validIds = new Set(validTeams.map(t => t.id))
      const invalid = teamIds.find(id => !validIds.has(id))
      if (invalid) {
        return res.status(400).json({ error: `team_id ${invalid} does not belong to this session` })
      }
    }

    // upsert each score
    for (const s of scores) {
      await pool.query(
        `INSERT INTO scores (team_id, round_id, points)
         VALUES ($1, $2, $3)
         ON CONFLICT (team_id, round_id)
         DO UPDATE SET points = EXCLUDED.points`,
        [s.team_id, roundId, s.points]
      )
    }

    const { rows: updatedScores } = await pool.query(
      'SELECT team_id, points FROM scores WHERE round_id = $1', [roundId]
    )

    res.json({ round_id: parseInt(roundId), scores: updatedScores })
  } catch (err) { next(err) }
})

module.exports = router
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
NODE_ENV=test npx jest tests/scores.test.js --verbose
```

- [ ] **Step 5: Commit**

```bash
git add src/routes/scores.js tests/scores.test.js
git commit -m "feat: scores API with upsert and tests"
```

---

## Task 8: Leaderboard API

**Files:**
- Modify: `src/routes/leaderboard.js`
- Create: `tests/leaderboard.test.js`

- [ ] **Step 1: Write the failing tests**

Create `tests/leaderboard.test.js`:

```js
process.env.NODE_ENV = 'test'

const request = require('supertest')
const app = require('../src/app')
const { truncateAll, closePool } = require('./setup')

beforeEach(truncateAll)
afterAll(closePool)

async function setup() {
  const { body: session } = await request(app).post('/sessions').send({ name: 'Quiz' })
  const { body: teamA } = await request(app).post(`/sessions/${session.id}/teams`).send({ name: 'Alpha' })
  const { body: teamB } = await request(app).post(`/sessions/${session.id}/teams`).send({ name: 'Beta' })
  const { body: round1 } = await request(app).post(`/sessions/${session.id}/rounds`).send({ label: 'R1' })
  const { body: round2 } = await request(app).post(`/sessions/${session.id}/rounds`).send({ label: 'R2' })
  return { session, teamA, teamB, round1, round2 }
}

describe('GET /sessions/:id/leaderboard', () => {
  it('returns teams sorted by total descending', async () => {
    const { session, teamA, teamB, round1 } = await setup()
    await request(app).post(`/rounds/${round1.id}/scores`).send({
      scores: [{ team_id: teamA.id, points: 5 }, { team_id: teamB.id, points: 10 }]
    })
    const res = await request(app).get(`/sessions/${session.id}/leaderboard`)
    expect(res.status).toBe(200)
    expect(res.body[0].name).toBe('Beta')
    expect(res.body[0].total).toBe(10)
    expect(res.body[1].name).toBe('Alpha')
    expect(res.body[1].total).toBe(5)
  })

  it('accumulates scores across rounds', async () => {
    const { session, teamA, round1, round2 } = await setup()
    await request(app).post(`/rounds/${round1.id}/scores`).send({ scores: [{ team_id: teamA.id, points: 6 }] })
    await request(app).post(`/rounds/${round2.id}/scores`).send({ scores: [{ team_id: teamA.id, points: 4 }] })
    const res = await request(app).get(`/sessions/${session.id}/leaderboard`)
    const alpha = res.body.find(t => t.name === 'Alpha')
    expect(alpha.total).toBe(10)
  })

  it('teams with no scores appear with total 0', async () => {
    const { session } = await setup()
    const res = await request(app).get(`/sessions/${session.id}/leaderboard`)
    expect(res.body.every(t => t.total === 0)).toBe(true)
  })

  it('ties broken alphabetically by name', async () => {
    const { session, teamA, teamB, round1 } = await setup()
    await request(app).post(`/rounds/${round1.id}/scores`).send({
      scores: [{ team_id: teamA.id, points: 7 }, { team_id: teamB.id, points: 7 }]
    })
    const res = await request(app).get(`/sessions/${session.id}/leaderboard`)
    expect(res.body[0].name).toBe('Alpha')  // A before B, same rank
    expect(res.body[0].rank).toBe(res.body[1].rank)  // dense rank — same rank for tied teams
  })

  it('returns 404 for unknown session', async () => {
    const res = await request(app).get('/sessions/99999/leaderboard')
    expect(res.status).toBe(404)
  })

  it('response includes rank, team_id, name, total', async () => {
    const { session } = await setup()
    const res = await request(app).get(`/sessions/${session.id}/leaderboard`)
    expect(res.body[0]).toMatchObject({
      rank: expect.any(Number),
      team_id: expect.any(Number),
      name: expect.any(String),
      total: expect.any(Number)
    })
  })
})
```

- [ ] **Step 2: Run tests — verify they fail**

```bash
NODE_ENV=test npx jest tests/leaderboard.test.js --verbose
```

- [ ] **Step 3: Implement leaderboard route**

Replace `src/routes/leaderboard.js`:

```js
const { Router } = require('express')
const pool = require('../db')
const router = Router()

// GET /sessions/:id/leaderboard
router.get('/:id/leaderboard', async (req, res, next) => {
  try {
    const { id } = req.params

    const { rows: sessions } = await pool.query(
      'SELECT id FROM sessions WHERE id = $1', [id]
    )
    if (!sessions.length) return res.status(404).json({ error: 'Session not found' })

    const { rows } = await pool.query(
      // DENSE_RANK window orders rank assignment; outer ORDER BY controls result row order
      // Both must use the same criteria: total DESC, name ASC
      `SELECT
         DENSE_RANK() OVER (ORDER BY COALESCE(SUM(s.points), 0) DESC, t.name ASC) AS rank,
         t.id AS team_id,
         t.name,
         COALESCE(SUM(s.points), 0) AS total
       FROM teams t
       LEFT JOIN scores s ON s.team_id = t.id
       WHERE t.session_id = $1
       GROUP BY t.id, t.name
       ORDER BY total DESC, t.name ASC`,
      [id]
    )

    // cast rank and total to numbers (pg returns them as strings)
    res.json(rows.map(r => ({
      rank: Number(r.rank),
      team_id: r.team_id,
      name: r.name,
      total: Number(r.total)
    })))
  } catch (err) { next(err) }
})

module.exports = router
```

- [ ] **Step 4: Run tests — verify they pass**

```bash
NODE_ENV=test npx jest tests/leaderboard.test.js --verbose
```

- [ ] **Step 5: Run the full test suite**

```bash
NODE_ENV=test npx jest --verbose
```

Expected: all tests PASS. If anything fails, fix before continuing.

- [ ] **Step 6: Commit**

```bash
git add src/routes/leaderboard.js tests/leaderboard.test.js
git commit -m "feat: leaderboard API with dense rank and tests"
```

---

## Task 9: Frontend — Home Page

**Files:**
- Create: `public/index.html`
- Create: `public/style.css`
- Create: `public/home.js`

No tests for frontend — manual verification.

- [ ] **Step 1: Create public/style.css**

```css
* { box-sizing: border-box; margin: 0; padding: 0; }

body {
  font-family: system-ui, sans-serif;
  background: #f5f5f5;
  color: #222;
  padding: 2rem;
}

h1 { font-size: 1.8rem; margin-bottom: 1.5rem; }
h2 { font-size: 1.2rem; margin-bottom: 1rem; }

.card {
  background: white;
  border-radius: 8px;
  padding: 1.5rem;
  margin-bottom: 1.5rem;
  box-shadow: 0 1px 4px rgba(0,0,0,0.1);
}

form { display: flex; gap: 0.5rem; flex-wrap: wrap; }

input[type="text"] {
  flex: 1;
  padding: 0.5rem 0.75rem;
  border: 1px solid #ccc;
  border-radius: 4px;
  font-size: 1rem;
}

button {
  padding: 0.5rem 1.25rem;
  background: #2563eb;
  color: white;
  border: none;
  border-radius: 4px;
  font-size: 1rem;
  cursor: pointer;
}

button:hover { background: #1d4ed8; }
button:disabled { background: #9ca3af; cursor: not-allowed; }

table { width: 100%; border-collapse: collapse; }
th, td { text-align: left; padding: 0.6rem 0.75rem; border-bottom: 1px solid #eee; }
th { font-weight: 600; background: #f9f9f9; }
tr:hover td { background: #f0f4ff; }

a { color: #2563eb; text-decoration: none; }
a:hover { text-decoration: underline; }

.badge {
  display: inline-block;
  padding: 0.2rem 0.6rem;
  border-radius: 12px;
  font-size: 0.8rem;
  font-weight: 600;
}

.badge-active { background: #dcfce7; color: #15803d; }
.badge-finished { background: #f1f5f9; color: #64748b; }

.error { color: #dc2626; font-size: 0.9rem; margin-top: 0.5rem; }
```

- [ ] **Step 2: Create public/index.html**

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Pub Quiz Scoreboard</title>
  <link rel="stylesheet" href="/style.css">
</head>
<body>
  <h1>🍺 Pub Quiz Scoreboard</h1>

  <div class="card">
    <h2>New Session</h2>
    <form id="create-form">
      <input type="text" id="session-name" placeholder="Quiz name..." required>
      <button type="submit">Create</button>
    </form>
    <p class="error" id="create-error"></p>
  </div>

  <div class="card">
    <h2>Sessions</h2>
    <table>
      <thead>
        <tr><th>Name</th><th>Status</th><th>Created</th><th></th></tr>
      </thead>
      <tbody id="sessions-list">
        <tr><td colspan="4">Loading...</td></tr>
      </tbody>
    </table>
  </div>

  <script src="/home.js"></script>
</body>
</html>
```

- [ ] **Step 3: Create public/home.js**

```js
async function loadSessions() {
  const res = await fetch('/sessions')
  const sessions = await res.json()
  const tbody = document.getElementById('sessions-list')
  if (!sessions.length) {
    tbody.innerHTML = '<tr><td colspan="4">No sessions yet.</td></tr>'
    return
  }
  tbody.innerHTML = sessions.map(s => `
    <tr>
      <td><a href="/session.html?id=${s.id}">${s.name}</a></td>
      <td>
        <span class="badge ${s.finished_at ? 'badge-finished' : 'badge-active'}">
          ${s.finished_at ? 'Finished' : 'Active'}
        </span>
      </td>
      <td>${new Date(s.created_at).toLocaleDateString()}</td>
      <td><a href="/session.html?id=${s.id}">Open →</a></td>
    </tr>
  `).join('')
}

document.getElementById('create-form').addEventListener('submit', async (e) => {
  e.preventDefault()
  const name = document.getElementById('session-name').value.trim()
  const err = document.getElementById('create-error')
  err.textContent = ''
  const res = await fetch('/sessions', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name })
  })
  if (!res.ok) {
    const data = await res.json()
    err.textContent = data.error
    return
  }
  document.getElementById('session-name').value = ''
  loadSessions()
})

loadSessions()
```

- [ ] **Step 4: Smoke test the home page**

```bash
node src/index.js
```

Open `http://localhost:3000` — you should see the home page. Create a session. It should appear in the list. Click it (will 404 for now, session page coming next).

- [ ] **Step 5: Commit**

```bash
git add public/index.html public/style.css public/home.js
git commit -m "feat: home page with session list and create form"
```

---

## Task 10: Frontend — Session Page

**Files:**
- Create: `public/session.html`
- Create: `public/session.js`

- [ ] **Step 1: Create public/session.html**

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Pub Quiz Session</title>
  <link rel="stylesheet" href="/style.css">
  <style>
    .score-grid { width: 100%; border-collapse: collapse; margin-bottom: 1rem; }
    .score-grid th, .score-grid td { padding: 0.5rem; border-bottom: 1px solid #eee; }
    .score-grid input { width: 70px; padding: 0.3rem; border: 1px solid #ccc; border-radius: 4px; }
    .round-block { margin-bottom: 1.5rem; }
    .round-label { font-weight: 600; margin-bottom: 0.5rem; }
    #leaderboard table { width: 100%; }
    .rank-1 td { font-weight: bold; }
    .finished-msg { color: #64748b; font-style: italic; margin-top: 0.5rem; }
    #end-quiz { background: #dc2626; margin-top: 1rem; }
    #end-quiz:hover { background: #b91c1c; }
  </style>
</head>
<body>
  <p><a href="/">← All Sessions</a></p>
  <h1 id="session-title">Loading...</h1>

  <div style="display:grid; grid-template-columns: 1fr 1fr; gap: 1.5rem;">
    <div>
      <div class="card" id="teams-section">
        <h2>Teams</h2>
        <form id="add-team-form">
          <input type="text" id="team-name" placeholder="Team name...">
          <button type="submit">Add Team</button>
        </form>
        <p class="error" id="team-error"></p>
        <div id="teams-list" style="margin-top:1rem;"></div>
      </div>

      <div class="card" id="rounds-section">
        <h2>Rounds & Scores</h2>
        <form id="add-round-form">
          <input type="text" id="round-label" placeholder="Round label (optional)">
          <button type="submit">Add Round</button>
        </form>
        <p class="error" id="round-error"></p>
        <div id="rounds-list" style="margin-top:1rem;"></div>
      </div>
    </div>

    <div>
      <div class="card" id="leaderboard">
        <h2>Leaderboard</h2>
        <div id="leaderboard-table">Loading...</div>
      </div>
      <button id="end-quiz">End Quiz</button>
      <p class="finished-msg" id="finished-msg" style="display:none;">This session is finished.</p>
    </div>
  </div>

  <script src="/session.js"></script>
</body>
</html>
```

> **Finish state rendering:** When `session.finished_at` is set, the `render()` function must disable ALL interactive elements: the add-team input+button, the add-round input+button, every score input, every "Submit Scores" button, and the "End Quiz" button. The page reloads on finish (calling `loadSession()` again), so this is handled by the same `render()` path — just check `const finished = !!session.finished_at` at the top of render and pass `disabled` to each element conditionally. Because rounds and score inputs are generated dynamically inside `render()` via `innerHTML`, the `${finished ? 'disabled' : ''}` attribute must be embedded in the template literal for each input and button — there is no separate "disable everything" step after render. The session.js code below already does this correctly.

- [ ] **Step 2: Create public/session.js**

```js
const sessionId = new URLSearchParams(location.search).get('id')
if (!sessionId) location.href = '/'

let sessionData = null

async function loadSession() {
  const [sessionRes, lbRes] = await Promise.all([
    fetch(`/sessions/${sessionId}`),
    fetch(`/sessions/${sessionId}/leaderboard`)
  ])
  sessionData = await sessionRes.json()
  const leaderboard = await lbRes.json()
  render(sessionData, leaderboard)
}

function render(session, leaderboard) {
  const finished = !!session.finished_at

  // Title
  document.getElementById('session-title').innerHTML =
    `${session.name} ${finished ? '<span class="badge badge-finished">Finished</span>' : '<span class="badge badge-active">Active</span>'}`

  // Teams
  const teamsList = document.getElementById('teams-list')
  teamsList.innerHTML = session.teams.length
    ? `<table><thead><tr><th>Team</th><th>Total</th></tr></thead><tbody>
        ${session.teams.map(t => {
          const lb = leaderboard.find(l => l.team_id === t.id)
          return `<tr><td>${t.name}</td><td>${lb ? lb.total : 0}</td></tr>`
        }).join('')}
      </tbody></table>`
    : '<p>No teams yet.</p>'

  document.getElementById('add-team-form').querySelector('input').disabled = finished
  document.getElementById('add-team-form').querySelector('button').disabled = finished

  // Rounds
  const roundsList = document.getElementById('rounds-list')
  if (!session.rounds.length) {
    roundsList.innerHTML = '<p>No rounds yet.</p>'
  } else {
    roundsList.innerHTML = session.rounds.map(round => {
      const scoreMap = {}
      round.scores.forEach(s => { scoreMap[s.team_id] = s.points })
      return `
        <div class="round-block" data-round-id="${round.id}">
          <div class="round-label">Round ${round.number}${round.label ? ': ' + round.label : ''}</div>
          <table class="score-grid">
            <thead><tr><th>Team</th><th>Points</th></tr></thead>
            <tbody>
              ${session.teams.map(t => `
                <tr>
                  <td>${t.name}</td>
                  <td><input type="number" min="0" value="${scoreMap[t.id] ?? ''}"
                    data-team-id="${t.id}" ${finished ? 'disabled' : ''}></td>
                </tr>`).join('')}
            </tbody>
          </table>
          <button class="submit-scores-btn" data-round-id="${round.id}" ${finished ? 'disabled' : ''}>
            Submit Scores
          </button>
          <p class="error round-error-${round.id}"></p>
        </div>`
    }).join('')

    document.querySelectorAll('.submit-scores-btn').forEach(btn => {
      btn.addEventListener('click', submitScores)
    })
  }

  document.getElementById('add-round-form').querySelector('button').disabled = finished

  // End quiz button
  const endBtn = document.getElementById('end-quiz')
  endBtn.disabled = finished
  document.getElementById('finished-msg').style.display = finished ? 'block' : 'none'

  // Leaderboard
  renderLeaderboard(leaderboard)
}

function renderLeaderboard(leaderboard) {
  const el = document.getElementById('leaderboard-table')
  if (!leaderboard.length) { el.innerHTML = '<p>No teams yet.</p>'; return }
  el.innerHTML = `<table>
    <thead><tr><th>#</th><th>Team</th><th>Total</th></tr></thead>
    <tbody>
      ${leaderboard.map(t => `
        <tr class="${t.rank === 1 ? 'rank-1' : ''}">
          <td>${t.rank}</td><td>${t.name}</td><td>${t.total}</td>
        </tr>`).join('')}
    </tbody>
  </table>`
}

async function submitScores(e) {
  const roundId = e.target.dataset.roundId
  const block = document.querySelector(`[data-round-id="${roundId}"]`)
  const inputs = block.querySelectorAll('input[data-team-id]')
  const errEl = document.querySelector(`.round-error-${roundId}`)
  errEl.textContent = ''

  const scores = Array.from(inputs)
    .filter(i => i.value !== '')
    .map(i => ({ team_id: parseInt(i.dataset.teamId), points: parseInt(i.value) }))

  const res = await fetch(`/rounds/${roundId}/scores`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ scores })
  })

  if (!res.ok) {
    const data = await res.json()
    errEl.textContent = data.error
    return
  }

  // refresh leaderboard only
  const lbRes = await fetch(`/sessions/${sessionId}/leaderboard`)
  const leaderboard = await lbRes.json()
  renderLeaderboard(leaderboard)

  // update team totals in sidebar
  const teamsList = document.getElementById('teams-list')
  const rows = teamsList.querySelectorAll('tbody tr')
  rows.forEach(row => {
    const name = row.cells[0].textContent
    const lb = leaderboard.find(l => l.name === name)
    if (lb) row.cells[1].textContent = lb.total
  })
}

document.getElementById('add-team-form').addEventListener('submit', async (e) => {
  e.preventDefault()
  const name = document.getElementById('team-name').value.trim()
  const errEl = document.getElementById('team-error')
  errEl.textContent = ''
  const res = await fetch(`/sessions/${sessionId}/teams`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name })
  })
  if (!res.ok) { errEl.textContent = (await res.json()).error; return }
  document.getElementById('team-name').value = ''
  loadSession()
})

document.getElementById('add-round-form').addEventListener('submit', async (e) => {
  e.preventDefault()
  const label = document.getElementById('round-label').value.trim()
  const errEl = document.getElementById('round-error')
  errEl.textContent = ''
  const res = await fetch(`/sessions/${sessionId}/rounds`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ label: label || undefined })
  })
  if (!res.ok) { errEl.textContent = (await res.json()).error; return }
  document.getElementById('round-label').value = ''
  loadSession()
})

document.getElementById('end-quiz').addEventListener('click', async () => {
  if (!confirm('End this quiz session? No more scores can be added.')) return
  const res = await fetch(`/sessions/${sessionId}/finish`, { method: 'PATCH' })
  if (res.ok) loadSession()
})

loadSession()
```

- [ ] **Step 3: Smoke test the full app**

```bash
node src/index.js
```

Walk through the full flow manually:
1. Open `http://localhost:3000` — create a session
2. Open the session — add 2-3 teams
3. Add 2 rounds
4. Submit scores for each round
5. Verify leaderboard updates correctly
6. Click "End Quiz" — verify all inputs disable and badge shows "Finished"

- [ ] **Step 4: Commit**

```bash
git add public/session.html public/session.js
git commit -m "feat: session page with teams, rounds, scores, and leaderboard"
```

---

## Task 11: Final Check

- [ ] **Run full test suite one last time**

```bash
NODE_ENV=test npx jest --verbose
```

Expected: all tests PASS, no failures.

- [ ] **Verify app starts cleanly**

```bash
node src/index.js
```

Expected: `Pub Quiz running on http://localhost:3000`

- [ ] **Verify missing DATABASE_URL exits with clear error**

```bash
env -u DATABASE_URL node src/index.js
```

Expected: prints `ERROR: DATABASE_URL ... is not set.` and exits (non-zero exit code).
Note: `DATABASE_URL=""` sets an empty string which behaves differently — use `env -u` to truly unset it.

- [ ] **Final commit**

```bash
git add -A
git status  # make sure nothing sensitive is staged
git commit -m "feat: pub quiz scoreboard complete"
```

- [ ] **Notify Peppy for deployment**

Send Peppy a message with: repo location, port (3000), DATABASE_URL needed, migration command (`psql $DATABASE_URL < migrations/001_init.sql`), and desired subdomain.
