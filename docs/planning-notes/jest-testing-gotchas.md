# Jest Testing Gotchas for Plan Writers

Lessons learned from builds. Include these patterns in any Node.js + database plan.

---

## Shared pg Pool — Do NOT call `pool.end()` per test file

When tests share a singleton pg Pool (the standard pattern for Express apps), calling `pool.end()` inside `afterAll()` in one test file will kill the pool for all other test files running in the same Jest process.

**Wrong:**
```js
// tests/sessions.test.js
afterAll(() => pool.end())  // ❌ kills the pool for teams.test.js, rounds.test.js, etc.
```

**Right:**
```js
// package.json — jest config
"jest": {
  "testEnvironment": "node",
  "forceExit": true   // ✅ Jest force-exits after all suites complete
}
```

Or use a single global teardown file:
```js
// tests/globalTeardown.js
const pool = require('../src/db')
module.exports = async () => { await pool.end() }

// package.json
"jest": {
  "globalTeardown": "./tests/globalTeardown.js"
}
```

**Rule for plans:** Never include `afterAll(closePool)` or `afterAll(() => pool.end())` in individual test files. Use `forceExit: true` in jest config instead.
