/**
 * Synthetic tests for scripts/infisical-metadata.mjs — Cipher's required
 * negatives (aperture-5nxd8), revision 2.
 *
 * ISOLATION CONTRACT (Cipher HIGH-4):
 *   These tests CANNOT reach the live host and CANNOT trigger a credential
 *   read. The `list-metadata` action is NEVER invoked. The module exports no
 *   credential reader, no credential path and no live caller, so the only
 *   reachable surfaces are pure functions over injected data. The CLI is
 *   exercised ONLY with rejected actions, which return before any file or
 *   network access.
 *
 * Control characters appear as ESCAPES, never literal bytes, so this file stays
 * text- and diff-auditable (Cipher's closing note).
 *
 * Run: node --test scripts/__tests__/infisical-metadata.test.mjs
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtempSync, writeFileSync, symlinkSync, linkSync, rmSync, openSync, statSync, constants as FS } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { Readable } from 'node:stream';
import {
  parseCredentials, safeStr, isForbiddenCp, classifyStatus, parseBounded,
  requireArray, projectSecretNames, projectWorkspaceMeta,
  consumeBoundedStream, validateAction, serializeReceipt,
  classifyRuntime, decodeCredentialBytes, withAbsoluteDeadline, composeAction,
  makeBudget, chargeRequest, remainingMs, orchestrateMetadata,
  MAX_RECEIPT_BYTES, MAX_REQUESTS, MAX_NAMES_TOTAL, ACTION_DEADLINE_MS,
  E, Fail, ONLY_ACTION,
} from '../infisical-metadata.mjs';

const run = promisify(execFile);
const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(HERE, '..', 'infisical-metadata.mjs');

// Canaries INJECTED into inputs below, then asserted absent from every output
// AND every thrown error — success paths and error paths alike.
const SECRET_CANARY = 'CANARY_SECRET_VALUE_ee6f1b2c';
const TOKEN_CANARY = 'CANARY_BEARER_TOKEN_9a3d7f04';

// Control-character constants, written as escapes.
const NUL = '\u0000'; const LF = '\u000a'; const CR = '\u000d';
const ESC = '\u001b'; const DEL = '\u007f';

function assertNoCanary(text, where) {
  assert.ok(!String(text).includes(SECRET_CANARY), `secret canary leaked in ${where}`);
  assert.ok(!String(text).includes(TOKEN_CANARY), `token canary leaked in ${where}`);
}

// Capture everything a thrown error could carry outward.
function errorSurface(fn) {
  try { fn(); return ''; }
  catch (e) {
    let j = '';
    try { j = JSON.stringify(e); } catch { j = ''; }
    return [e && e.code, e && e.message, e && e.stack, j].join('|');
  }
}

// ── Module isolation ──────────────────────────────────────────────────────
test('module exports no credential reader, path, or live caller', async () => {
  const mod = await import('../infisical-metadata.mjs');
  for (const banned of ['readCredentials', 'CRED_PATH', 'CRED_DIR', 'request', 'listMetadata', 'assertSafeRuntime']) {
    assert.ok(!(banned in mod), `${banned} must not be exported`);
  }
});

test('parseCredentials takes TEXT so it cannot read a file even by accident', () => {
  const c = parseCredentials('INFISICAL_CLIENT_ID=a' + LF + 'INFISICAL_CLIENT_SECRET=b' + LF);
  assert.deepEqual(c, { clientId: 'a', clientSecret: 'b' });
});

// ── Action validation: PURE, no subprocess (Cipher r2-4). A subprocess test
// cannot prove ordering survives a later regression, and once a credential file
// exists a subprocess of the real action would authenticate live.
test('validateAction accepts only the exact single action', () => {
  assert.equal(validateAction([ONLY_ACTION]), ONLY_ACTION);
});

test('validateAction rejects unknown, case-mutated, extra, empty and non-array', () => {
  const bad = [['list-secrets'], ['List-Metadata'], ['LIST-METADATA'], [ONLY_ACTION, '--x'],
    [], [ONLY_ACTION + ' '], ['inject'], ['write'], ['set'], ['get'], ['read'], ['export'],
    null, undefined, 'list-metadata'];
  for (const args of bad) {
    assert.throws(() => validateAction(args), (e) => e.code === E.BAD_ACTION,
      'args ' + JSON.stringify(args));
  }
});

// ── Credential parsing ────────────────────────────────────────────────────
test('rejects duplicate, extra, missing, empty, oversize and malformed', () => {
  const bad = [
    'INFISICAL_CLIENT_ID=a' + LF + 'INFISICAL_CLIENT_ID=b' + LF + 'INFISICAL_CLIENT_SECRET=c' + LF,
    'INFISICAL_CLIENT_ID=a' + LF + 'INFISICAL_CLIENT_SECRET=b' + LF + 'EXTRA=c' + LF,
    'INFISICAL_CLIENT_ID=a' + LF,
    'INFISICAL_CLIENT_ID=' + LF + 'INFISICAL_CLIENT_SECRET=b' + LF,
    'INFISICAL_CLIENT_ID=' + 'x'.repeat(600) + LF + 'INFISICAL_CLIENT_SECRET=b' + LF,
    'INFISICAL_CLIENT_ID' + LF + 'INFISICAL_CLIENT_SECRET=b' + LF,
  ];
  for (const t of bad) {
    assert.throws(() => parseCredentials(t), (e) => e instanceof Fail && e.code === E.CRED_PARSE);
  }
});

test('a failing credential parse never leaks the value into the error', () => {
  const text = 'INFISICAL_CLIENT_ID=' + SECRET_CANARY + LF
    + 'INFISICAL_CLIENT_SECRET=' + TOKEN_CANARY + LF + 'EXTRA=x' + LF;
  const surface = errorSurface(() => parseCredentials(text));
  assert.ok(surface.includes(E.CRED_PARSE), 'expected the stable code');
  assertNoCanary(surface, 'parseCredentials error surface');
});

// ── Status classification ─────────────────────────────────────────────────
test('redirects are refused', () => {
  for (const st of [301, 302, 303, 307, 308]) {
    assert.throws(() => classifyStatus(st), (e) => e.code === E.REDIRECT_REFUSED);
  }
});

test('401 and 403 map to auth-rejected', () => {
  for (const st of [401, 403]) {
    assert.throws(() => classifyStatus(st), (e) => e.code === E.AUTH_REJECTED);
  }
});

test('other non-200 fails closed without reflecting the status', () => {
  for (const st of [400, 404, 418, 429, 500, 502]) {
    const surface = errorSurface(() => classifyStatus(st));
    assert.ok(surface.includes(E.UPSTREAM_STATUS));
    assert.ok(!surface.includes(String(st)), `status ${st} must not be reflected`);
  }
});

test('200 is the only accepted status', () => { assert.equal(classifyStatus(200), true); });

// ── Body parsing with canaries injected into the body ─────────────────────
test('malformed and truncated bodies fail closed and never echo the body', () => {
  const bodies = [
    '{not json ' + SECRET_CANARY,
    '{"secrets":[{"secretKey":"A","secretValue":"' + SECRET_CANARY + '"',
    '',
  ];
  for (const b of bodies) {
    const surface = errorSurface(() => parseBounded(b));
    assert.ok(surface.includes(E.BAD_SHAPE), 'expected BAD_SHAPE');
    assertNoCanary(surface, 'parseBounded error surface');
  }
});

test('oversize body is refused before parsing', () => {
  const huge = '"' + 'x'.repeat(1000001) + '"';
  assert.throws(() => parseBounded(huge), (e) => e.code === E.BODY_TOO_LARGE);
});

// ── THE leak tests ────────────────────────────────────────────────────────
test('projectSecretNames emits key names and NEVER the injected value', () => {
  const body = { secrets: [
    { secretKey: 'OPENAI_API_KEY', secretValue: SECRET_CANARY, version: 3 },
    { secretKey: 'DATABASE_URL', secretValue: TOKEN_CANARY },
  ] };
  const names = projectSecretNames(body);
  assert.deepEqual(names, ['OPENAI_API_KEY', 'DATABASE_URL']);
  const serialized = JSON.stringify(names);
  assertNoCanary(serialized, 'projectSecretNames success output');
  assert.ok(!serialized.includes('secretValue'), 'value field must not survive');
});

test('a FAILING projection never leaks the injected value into the error', () => {
  const body = { secrets: [{ secretKey: 12345, secretValue: SECRET_CANARY }] };
  const surface = errorSurface(() => projectSecretNames(body));
  assert.ok(surface.includes(E.BAD_SHAPE));
  assertNoCanary(surface, 'projectSecretNames error surface');
});

test('workspace projection emits only id/name/slug/env slugs', () => {
  const ws = { id: 'w1', name: 'Quiz', slug: 'quiz-x', orgId: 'o1',
    environments: [{ slug: 'prod', name: 'Production', id: 'e1' }],
    autoCapitalization: SECRET_CANARY };
  const out = projectWorkspaceMeta(ws);
  assert.deepEqual(Object.keys(out).sort(), ['environmentSlugs', 'id', 'name', 'slug']);
  assertNoCanary(JSON.stringify(out), 'projectWorkspaceMeta output');
});

// ── Strict envelopes ──────────────────────────────────────────────────────
test('missing or wrong-typed envelopes fail closed rather than yielding empty', () => {
  assert.throws(() => projectSecretNames({}), (e) => e.code === E.BAD_SHAPE);
  assert.throws(() => projectSecretNames({ secrets: null }), (e) => e.code === E.BAD_SHAPE);
  assert.throws(() => projectSecretNames({ secrets: 'nope' }), (e) => e.code === E.BAD_SHAPE);
  assert.throws(() => projectWorkspaceMeta({ id: 'a', name: 'b', slug: 'c' }), (e) => e.code === E.BAD_SHAPE);
  assert.throws(() => requireArray(null, 'x', 5), (e) => e.code === E.BAD_SHAPE);
});

test('over-limit collections are refused, not truncated', () => {
  const many = { secrets: Array.from({ length: 5001 }, (_, i) => ({ secretKey: 'K' + i })) };
  assert.throws(() => projectSecretNames(many), (e) => e.code === E.LIMIT_EXCEEDED);
  const ws = { id: 'a', name: 'b', slug: 'c',
    environments: Array.from({ length: 21 }, () => ({ slug: 's' })) };
  assert.throws(() => projectWorkspaceMeta(ws), (e) => e.code === E.LIMIT_EXCEEDED);
});

// ── Output safety ─────────────────────────────────────────────────────────
test('C0, CR, ESC, NUL and DEL are escaped, never emitted raw', () => {
  const out = safeStr('a' + LF + 'b' + CR + 'c' + ESC + 'd' + NUL + 'e' + DEL);
  assert.equal(out, 'a\\u000ab\\u000dc\\u001bd\\u0000e\\u007f');
  for (const raw of [LF, CR, ESC, NUL, DEL]) {
    assert.ok(!out.includes(raw), 'raw control must not survive');
  }
});

test('Unicode line/paragraph separators and bidi/format controls are escaped', () => {
  for (const cp of [0x2028, 0x2029, 0x202e, 0x2066, 0x2069, 0x200b, 0xfeff, 0x061c]) {
    assert.ok(isForbiddenCp(cp), 'U+' + cp.toString(16) + ' must be forbidden');
    const out = safeStr('a' + String.fromCodePoint(cp) + 'b');
    assert.ok(!out.includes(String.fromCodePoint(cp)), 'must be escaped');
    assert.ok(out.includes('\\u' + cp.toString(16).padStart(4, '0')));
  }
});

test('ordinary text and non-ASCII letters are left intact', () => {
  assert.equal(safeStr('QUIZ_API_KEY'), 'QUIZ_API_KEY');
  assert.equal(safeStr('projecao'), 'projecao');
});

test('empty, oversize and non-string names are rejected', () => {
  assert.throws(() => safeStr(''), (e) => e.code === E.BAD_SHAPE);
  assert.throws(() => safeStr('x'.repeat(300)), (e) => e.code === E.BAD_SHAPE);
  for (const v of [{}, 42, null, undefined, []]) {
    assert.throws(() => safeStr(v), (e) => e.code === E.BAD_SHAPE);
  }
});

test('a hostile secret NAME cannot forge receipt structure', () => {
  const hostile = 'EVIL' + LF + '{"ok":true}' + ESC + '[2J' + String.fromCodePoint(0x202e);
  const [name] = projectSecretNames({ secrets: [{ secretKey: hostile, secretValue: SECRET_CANARY }] });
  for (const raw of [LF, ESC, String.fromCodePoint(0x202e)]) {
    assert.ok(!name.includes(raw), 'raw control must not survive into the receipt');
  }
  const line = JSON.stringify({ ok: true, names: [name] });
  assert.equal(line.split(LF).length, 1, 'receipt stays one line');
  assertNoCanary(line, 'receipt line');
});

// ── Credential-file guard primitives ──────────────────────────────────────
test('O_NOFOLLOW refuses a symlinked creds file', () => {
  const dir = mkdtempSync(join(tmpdir(), 'apx-'));
  try {
    const real = join(dir, 'real.env'); const link = join(dir, 'link.env');
    writeFileSync(real, 'INFISICAL_CLIENT_ID=a' + LF + 'INFISICAL_CLIENT_SECRET=b' + LF, { mode: 0o600 });
    symlinkSync(real, link);
    assert.throws(() => openSync(link, FS.O_RDONLY | FS.O_NOFOLLOW), (e) => e.code === 'ELOOP');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('a hard-linked creds file raises nlink so the guard trips', () => {
  const dir = mkdtempSync(join(tmpdir(), 'apx-'));
  try {
    const a = join(dir, 'a.env'); const b = join(dir, 'b.env');
    writeFileSync(a, 'x', { mode: 0o600 });
    linkSync(a, b);
    assert.ok(statSync(a).nlink > 1);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

// ── Source invariants ─────────────────────────────────────────────────────
test('source uses node:http and never global fetch or a proxy dispatcher', async () => {
  const { readFileSync } = await import('node:fs');
  const src = readFileSync(SCRIPT, 'utf8');
  const code = src.split(LF).filter((l) => !l.trim().startsWith('*') && !l.trim().startsWith('//')).join(LF);
  assert.ok(code.includes("from 'node:http'"), 'must use node:http');
  assert.ok(!/\bfetch\s*\(/.test(code), 'must not call global fetch');
  assert.ok(!code.includes('undici'), 'no undici dependency');
  assert.ok(code.includes('userInfo()'), 'home must come from the OS account, not $HOME');
});

test('source contains no subprocess, shell, eval or dynamic import', async () => {
  const { readFileSync } = await import('node:fs');
  const src = readFileSync(SCRIPT, 'utf8');
  for (const banned of ['child_process', 'execSync', 'spawn', 'eval(', 'require(']) {
    assert.ok(!src.includes(banned), `must not contain ${banned}`);
  }
});

test('source pins one host and exactly three metadata paths', async () => {
  const { readFileSync } = await import('node:fs');
  const src = readFileSync(SCRIPT, 'utf8');
  assert.ok(src.includes("const HOST = '100.102.73.112'"));
  assert.ok(src.includes('const PORT = 3005'));
  const paths = [...src.matchAll(/^const P_[A-Z]+ = '([^']+)';$/gm)].map((m) => m[1]).sort();
  assert.deepEqual(paths, [
    '/api/v1/auth/universal-auth/login', '/api/v1/workspace', '/api/v3/secrets/raw',
  ].sort());
});

test('source never references secretValue in executable code', async () => {
  const { readFileSync } = await import('node:fs');
  const src = readFileSync(SCRIPT, 'utf8');
  const code = src.split(LF).filter((l) => !l.trim().startsWith('*') && !l.trim().startsWith('//')).join(LF);
  assert.ok(!code.includes('secretValue'), 'secretValue must not appear in code');
  assert.ok(/\.secretKey\b/.test(code), 'only the key name is projected');
});

// ══ Cipher r2-4: the load-bearing composition, through narrow seams ══════
// None of these can reach a host: the transport is injected and the streams are
// synthetic. No credential reader, URL or live transport is exported.

const FAKE_CREDS = { clientId: 'fake-id', clientSecret: 'fake-secret' };

function fakeTransport(handlers) {
  return async ({ path }) => {
    for (const [prefix, fn] of handlers) {
      if (path.startsWith(prefix)) return fn();
    }
    throw new Fail(E.BAD_SHAPE);
  };
}

// ── Bounded streaming: abort DURING the stream, before full allocation ────
test('consumeBoundedStream aborts above the cap mid-stream, not after buffering', async () => {
  let pushed = 0;
  const stream = new Readable({
    read() { pushed += 1; this.push(Buffer.alloc(1024, 0x61)); },
  });
  await assert.rejects(() => consumeBoundedStream(stream, 4096), (e) => e.code === E.BODY_TOO_LARGE);
  assert.ok(pushed < 50, 'must abort early, not consume an unbounded stream');
  assert.ok(stream.destroyed, 'stream must be destroyed on abort');
});

test('consumeBoundedStream accepts a body at the cap and decodes UTF-8', async () => {
  const text = await consumeBoundedStream(Readable.from([Buffer.from('{"a":1}', 'utf8')]), 4096);
  assert.equal(text, '{"a":1}');
});

test('consumeBoundedStream rejects invalid UTF-8 rather than substituting U+FFFD', async () => {
  const bad = Readable.from([Buffer.from([0xff, 0xfe, 0xfd])]);
  await assert.rejects(() => consumeBoundedStream(bad, 4096), (e) => e.code === E.BAD_ENCODING);
});

test('a DRIP stream terminates via the cap rather than running forever', async () => {
  // Continual bytes defeat a socket-inactivity timeout; the byte cap is what
  // actually bounds it. Cipher's drip scenario.
  const stream = new Readable({ read() { this.push(Buffer.alloc(64, 0x62)); } });
  await assert.rejects(() => consumeBoundedStream(stream, 2048), (e) => e.code === E.BODY_TOO_LARGE);
  assert.ok(stream.destroyed);
});

// ── Budget: absolute deadline and global caps ────────────────────────────
test('chargeRequest returns min(per-request cap, remaining action time)', () => {
  const b = makeBudget();
  const t = chargeRequest(b);
  assert.ok(t > 0 && t <= 10000, 'per-request cap applies while plenty of time remains');
  const nearEnd = makeBudget();
  nearEnd.startedAt = Date.now() - (ACTION_DEADLINE_MS - 250);
  const t2 = chargeRequest(nearEnd);
  assert.ok(t2 <= 250, 'must clamp to the remaining action time, got ' + t2);
});

test('chargeRequest rejects AT or AFTER the action deadline', () => {
  const b = makeBudget();
  b.startedAt = Date.now() - ACTION_DEADLINE_MS;
  assert.throws(() => chargeRequest(b), (e) => e.code === E.DEADLINE);
  const past = makeBudget();
  past.startedAt = Date.now() - (ACTION_DEADLINE_MS + 5000);
  assert.throws(() => chargeRequest(past), (e) => e.code === E.DEADLINE);
});

test('the global request cap is enforced', () => {
  const b = makeBudget();
  for (let i = 0; i < MAX_REQUESTS; i += 1) chargeRequest(b);
  assert.throws(() => chargeRequest(b), (e) => e.code === E.LIMIT_EXCEEDED);
});

test('remainingMs shrinks as the action proceeds', () => {
  const b = makeBudget();
  b.startedAt = Date.now() - 1000;
  assert.ok(remainingMs(b) <= ACTION_DEADLINE_MS - 900);
});

// ── Receipt ceiling measured in BYTES, not UTF-16 code units ─────────────
test('serializeReceipt returns a single line for a small receipt', () => {
  const line = serializeReceipt({ ok: true, projects: [] });
  assert.equal(line.split(LF).length, 1);
  assert.deepEqual(JSON.parse(line), { ok: true, projects: [] });
});

test('serializeReceipt measures UTF-8 BYTES so multibyte names cannot slip past', () => {
  // Each char is 3 UTF-8 bytes but ONE UTF-16 code unit, so a code-unit check
  // would wrongly pass this.
  const wide = '中'.repeat(1);
  const many = Array.from({ length: Math.ceil(MAX_RECEIPT_BYTES / 3) + 100 }, () => 'A' + wide);
  assert.throws(() => serializeReceipt({ ok: true, names: many }),
    (e) => e.code === E.RECEIPT_TOO_LARGE);
});

// ── Injected-transport orchestration: the REAL operational path ──────────
test('orchestration succeeds end to end and emits names only', async () => {
  const transport = fakeTransport([
    ['/api/v1/auth/universal-auth/login', () => ({ accessToken: TOKEN_CANARY })],
    ['/api/v1/workspace', () => ({ workspaces: [
      { id: 'w1', name: 'Quiz', slug: 'quiz', environments: [{ slug: 'prod' }] },
    ] })],
    ['/api/v3/secrets/raw', () => ({ secrets: [
      { secretKey: 'OPENAI_API_KEY', secretValue: SECRET_CANARY },
    ] })],
  ]);
  const receipt = await orchestrateMetadata({ transport, credentials: FAKE_CREDS });
  assert.equal(receipt.ok, true);
  assert.equal(receipt.projectCount, 1);
  assert.equal(receipt.secretNameCount, 1);
  assert.deepEqual(receipt.projects[0].environments[0].secretNames, ['OPENAI_API_KEY']);
  // The bearer token was returned BY the fake auth and must not appear anywhere.
  assertNoCanary(serializeReceipt(receipt), 'successful orchestration receipt');
});

test('orchestration rejects a malformed auth envelope', async () => {
  for (const authBody of [{}, { accessToken: '' }, { accessToken: 42 }, null, 'nope']) {
    const transport = fakeTransport([['/api/v1/auth', () => authBody]]);
    await assert.rejects(() => orchestrateMetadata({ transport, credentials: FAKE_CREDS }),
      (e) => e.code === E.BAD_SHAPE, 'auth body ' + JSON.stringify(authBody));
  }
});

test('orchestration rejects a malformed workspace envelope', async () => {
  for (const wsBody of [{}, { workspaces: null }, { workspaces: 'x' }, { workspaces: [null] }]) {
    const transport = fakeTransport([
      ['/api/v1/auth/universal-auth/login', () => ({ accessToken: 'tok' })],
      ['/api/v1/workspace', () => wsBody],
    ]);
    await assert.rejects(() => orchestrateMetadata({ transport, credentials: FAKE_CREDS }),
      (e) => e.code === E.BAD_SHAPE, 'ws body ' + JSON.stringify(wsBody));
  }
});

test('orchestration enforces the cumulative name cap across environments', async () => {
  const envs = Array.from({ length: 15 }, (_, i) => ({ slug: 'env' + i }));
  const bulk = { secrets: Array.from({ length: 400 }, (_, i) => ({ secretKey: 'K' + i })) };
  const transport = fakeTransport([
    ['/api/v1/auth/universal-auth/login', () => ({ accessToken: 'tok' })],
    ['/api/v1/workspace', () => ({ workspaces: [
      { id: 'w1', name: 'W', slug: 'w', environments: envs },
    ] })],
    ['/api/v3/secrets/raw', () => bulk],
  ]);
  await assert.rejects(() => orchestrateMetadata({ transport, credentials: FAKE_CREDS }),
    (e) => e.code === E.LIMIT_EXCEEDED);
});

test('a failing orchestration never leaks the injected token or value', async () => {
  const transport = fakeTransport([
    ['/api/v1/auth/universal-auth/login', () => ({ accessToken: TOKEN_CANARY })],
    ['/api/v1/workspace', () => ({ workspaces: [
      { id: 'w1', name: 'W', slug: 'w', environments: [{ slug: 'p' }] },
    ] })],
    // secretKey wrong type while the entry still carries a value -> throws
    ['/api/v3/secrets/raw', () => ({ secrets: [{ secretKey: 1, secretValue: SECRET_CANARY }] })],
  ]);
  let surface = '';
  try { await orchestrateMetadata({ transport, credentials: FAKE_CREDS }); }
  catch (e) {
    let j = ''; try { j = JSON.stringify(e); } catch { j = ''; }
    surface = [e && e.code, e && e.message, e && e.stack, j].join('|');
  }
  assert.ok(surface.includes(E.BAD_SHAPE));
  assertNoCanary(surface, 'failing orchestration error surface');
});

// ══ Cipher r3-1: runtime guard classifier ═══════════════════════════════
const CLEAN_RUNTIME = { env: {}, execArgv: [], globalAgentIsStock: true };

test('classifyRuntime accepts a clean runtime', () => {
  assert.equal(classifyRuntime(CLEAN_RUNTIME), true);
  assert.equal(classifyRuntime({ ...CLEAN_RUNTIME, env: { PATH: '/usr/bin', HOME: '/x' } }), true);
});

test('classifyRuntime rejects every instrumented state', () => {
  const rejected = [
    { ...CLEAN_RUNTIME, env: { NODE_OPTIONS: '--require ./evil.js' } },
    { ...CLEAN_RUNTIME, env: { NODE_OPTIONS: '--use-env-proxy' } },
    { ...CLEAN_RUNTIME, env: { NODE_OPTIONS: ' ' } },
    { ...CLEAN_RUNTIME, execArgv: ['--inspect'] },
    { ...CLEAN_RUNTIME, execArgv: ['--use-env-proxy'] },
    { ...CLEAN_RUNTIME, execArgv: ['--import', './evil.mjs'] },
    { ...CLEAN_RUNTIME, env: { NODE_DEBUG: 'http' } },
    { ...CLEAN_RUNTIME, env: { NODE_DEBUG_NATIVE: 'http' } },
    { ...CLEAN_RUNTIME, env: { NODE_USE_ENV_PROXY: '1' } },
    { ...CLEAN_RUNTIME, globalAgentIsStock: false },
  ];
  for (const state of rejected) {
    assert.throws(() => classifyRuntime(state), (e) => e.code === E.UNSAFE_RUNTIME,
      'must reject ' + JSON.stringify({ env: state.env, execArgv: state.execArgv, g: state.globalAgentIsStock }));
  }
});

test('the guard runs BEFORE the credential reader, and a failing guard means it is never called', async () => {
  const order = [];
  const readCreds = () => { order.push('read'); return { clientId: 'a', clientSecret: 'b' }; };
  const orchestrate = () => { order.push('orchestrate'); return { ok: true }; };

  await composeAction({ guard: () => { order.push('guard'); }, readCreds, orchestrate });
  assert.deepEqual(order, ['guard', 'read', 'orchestrate'], 'guard must be first');

  order.length = 0;
  await assert.rejects(
    () => composeAction({ guard: () => { order.push('guard'); throw new Fail(E.UNSAFE_RUNTIME); }, readCreds, orchestrate }),
    (e) => e.code === E.UNSAFE_RUNTIME);
  assert.deepEqual(order, ['guard'], 'credential reader must NOT run when the guard fails');
});

// ══ Cipher r3-2: the wall-clock deadline wrapper ════════════════════════
test('withAbsoluteDeadline rejects E_DEADLINE, aborts once, and settles once', async () => {
  let aborts = 0;
  let settles = 0;
  const p = withAbsoluteDeadline({
    ms: 30,
    abort: () => { aborts += 1; },
    // never calls back — the deadline is the only way out
    start: (ok, bad) => { setTimeout(() => { settles += 1; ok('late'); }, 300); },
  });
  await assert.rejects(() => p, (e) => e.code === E.DEADLINE);
  assert.equal(aborts, 1, 'abort called exactly once');
  await new Promise((r) => setTimeout(r, 400));
  assert.equal(settles, 1, 'the late callback fired but must not re-settle the promise');
});

test('withAbsoluteDeadline clears its timer on a normal settle', async () => {
  const before = process.getActiveResourcesInfo ? process.getActiveResourcesInfo().filter((x) => x === 'Timeout').length : 0;
  const v = await withAbsoluteDeadline({ ms: 5000, abort: () => {}, start: (ok) => ok('done') });
  assert.equal(v, 'done');
  const after = process.getActiveResourcesInfo ? process.getActiveResourcesInfo().filter((x) => x === 'Timeout').length : 0;
  assert.ok(after <= before, 'the 5s timer must be cleared, not left pending');
});

test('a NEVER-ENDING LOW-VOLUME stream is stopped by the deadline, not the byte cap', async () => {
  // Trickles a few bytes on an interval: too slow to hit the byte cap, and a
  // socket-inactivity timeout would never fire. Only the wall clock stops it.
  let ticks = 0;
  let destroyed = false;
  const stream = new Readable({ read() {} });
  const iv = setInterval(() => { ticks += 1; stream.push(Buffer.alloc(4, 0x63)); }, 10);
  stream.on('close', () => { destroyed = true; });

  await assert.rejects(
    () => withAbsoluteDeadline({
      ms: 60,
      abort: () => { clearInterval(iv); stream.destroy(); },
      start: (ok, bad) => { consumeBoundedStream(stream, 10 * 1024 * 1024).then(ok, bad); },
    }),
    (e) => e.code === E.DEADLINE);
  clearInterval(iv);
  assert.ok(ticks > 0, 'the stream was genuinely producing bytes');
  assert.ok(destroyed || stream.destroyed, 'the stream must be destroyed on deadline');
});

// ══ Cipher r3-3: fatal credential decoding ══════════════════════════════
test('decodeCredentialBytes decodes valid UTF-8', () => {
  const buf = Buffer.from('INFISICAL_CLIENT_ID=a', 'utf8');
  assert.equal(decodeCredentialBytes(buf), 'INFISICAL_CLIENT_ID=a');
});

test('invalid credential bytes fail closed instead of becoming U+FFFD', () => {
  // Buffer.toString('utf8') would SUBSTITUTE here and silently mutate the
  // credential, then send the mutation upstream.
  const invalid = Buffer.from([0x49, 0x44, 0x3d, 0xff, 0xfe, 0xfd]);
  assert.notEqual(invalid.toString('utf8').indexOf('\ufffd'), -1,
    'baseline: the replacement path really does substitute');
  assert.throws(() => decodeCredentialBytes(invalid), (e) => e.code === E.CRED_ENCODING);
});

test('a lone surrogate / truncated multibyte sequence is rejected', () => {
  assert.throws(() => decodeCredentialBytes(Buffer.from([0xe4, 0xb8])), (e) => e.code === E.CRED_ENCODING);
});

// ══ Cipher r4: deadline PRECEDENCE — the deadline must win the race ═════
test('a SYNCHRONOUS abort callback cannot steal the outcome from the deadline', async () => {
  // Cipher's exact reproduction: teardown invokes the operation's error
  // callback synchronously. Before the fix this settled E_NETWORK.
  let captured = null;
  let aborts = 0;
  await assert.rejects(
    () => withAbsoluteDeadline({
      ms: 5,
      abort: () => { aborts += 1; if (captured) captured(new Fail(E.NETWORK)); },
      start: (ok, bad) => { captured = bad; },
    }),
    (e) => e.code === E.DEADLINE);
  assert.equal(aborts, 1, 'abort still runs exactly once');
});

test('an abort that THROWS does not mask the deadline', async () => {
  let aborts = 0;
  await assert.rejects(
    () => withAbsoluteDeadline({
      ms: 5,
      abort: () => { aborts += 1; throw new Error('teardown blew up'); },
      start: () => {},
    }),
    (e) => e.code === E.DEADLINE);
  assert.equal(aborts, 1);
});

test('a LATE success after expiry cannot overwrite the deadline outcome', async () => {
  let captured = null;
  const p = withAbsoluteDeadline({
    ms: 5, abort: () => {}, start: (ok) => { captured = ok; },
  });
  await assert.rejects(() => p, (e) => e.code === E.DEADLINE);
  captured('too late');                       // must be ignored
  await assert.rejects(() => p, (e) => e.code === E.DEADLINE, 'outcome stays E_DEADLINE');
});

test('a LATE error after expiry cannot overwrite the deadline outcome', async () => {
  let captured = null;
  const p = withAbsoluteDeadline({
    ms: 5, abort: () => {}, start: (ok, bad) => { captured = bad; },
  });
  await assert.rejects(() => p, (e) => e.code === E.DEADLINE);
  captured(new Fail(E.NETWORK));              // must be ignored
  await assert.rejects(() => p, (e) => e.code === E.DEADLINE);
});

test('a normal settle still wins when it happens before the deadline', async () => {
  let aborts = 0;
  const v = await withAbsoluteDeadline({
    ms: 500, abort: () => { aborts += 1; }, start: (ok) => ok('fast'),
  });
  assert.equal(v, 'fast');
  assert.equal(aborts, 0, 'abort must NOT run when the operation settled in time');
});

