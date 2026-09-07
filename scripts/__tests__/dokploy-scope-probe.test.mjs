/**
 * Synthetic tests for scripts/dokploy-scope-probe.mjs (aperture-ztid5).
 *
 * ISOLATION: no live host, no real token, no SSH child, no Dokploy call. The
 * module exports no file reader, no live transport and no runnable action.
 * Composed behaviour is proven through the INJECTED orchestration seam, not by
 * grepping source — an earlier revision had a source-string test that passed
 * while the code violated the very property it claimed to guard.
 *
 * Control characters appear as ESCAPES, never literal bytes.
 * Run: node --test scripts/__tests__/dokploy-scope-probe.test.mjs
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFile, execFileSync } from 'node:child_process';
import { promisify } from 'node:util';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Readable } from 'node:stream';

import {
  parseTokenFile, classifyStatus, parseBounded, consumeBoundedStream,
  withAbsoluteDeadline, classifyRuntime, assertSafeRuntime, assertQuizScope,
  orchestrate, SSH_ARGS, E, Fail, OK_RECEIPT,
  QUIZ_PROJECT_ID, QUIZ_ORG_ID, TOKEN_KEY, P_PROJECT_ONE,
  classifyOwnedDir, buildRequestOptions,
} from '../dokploy-scope-probe.mjs';

const run = promisify(execFile);
const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(HERE, '..', 'dokploy-scope-probe.mjs');
const SRC = readFileSync(SCRIPT, 'utf8');
const CODE = SRC.split('\n').filter((l) => !l.trim().startsWith('*') && !l.trim().startsWith('//')).join('\n');

const TOKEN_CANARY = 'CANARY_DOKPLOY_TOKEN_5b2e9a71';
const LF = '\u000a';
const GOOD = TOKEN_KEY + '=' + TOKEN_CANARY + LF;

function surface(e) {
  let j = ''; try { j = JSON.stringify(e); } catch { j = ''; }
  return [e && e.code, e && e.message, e && e.stack, j].join('|');
}
function caught(fn) { try { fn(); return null; } catch (e) { return e; } }

// ══ COMPOSED SEQUENCE — the gate that actually matters ═══════════════════
function harness({ body, tokenText = GOOD, failForward = false } = {}) {
  const calls = [];
  const seen = [];
  let cleaned = 0;
  return {
    calls, seen, cleanedCount: () => cleaned,
    args: {
      assertContextFn: () => { calls.push('context'); return true; },
      readTokenFn: () => { calls.push('readToken'); return parseTokenFile(tokenText); },
      openForwardFn: () => {
        calls.push('openForward');
        return {
          ready: failForward
            ? Promise.reject(new Fail(E.FORWARD_FAILED))
            : Promise.resolve('/fake/socket'),
          cleanup: () => { cleaned += 1; },
        };
      },
      requestFn: (opts) => { calls.push('request'); seen.push(opts); return Promise.resolve(body); },
    },
  };
}

test('exact sequence: context -> token -> forward -> ONE request', async () => {
  const h = harness({ body: { projectId: QUIZ_PROJECT_ID, organizationId: QUIZ_ORG_ID } });
  const receipt = await orchestrate(h.args);
  assert.equal(receipt, OK_RECEIPT);
  assert.deepEqual(h.calls, ['context', 'readToken', 'openForward', 'request']);
  assert.equal(h.seen.length, 1, 'EXACTLY one request may cross the forward');
  assert.equal(h.cleanedCount(), 1, 'forward cleaned up exactly once');
});

test('SSH context is asserted BEFORE the token is ever read', async () => {
  const calls = [];
  await assert.rejects(() => orchestrate({
    assertContextFn: () => { calls.push('context'); throw new Fail(E.SSH_CONTEXT); },
    readTokenFn: () => { calls.push('readToken'); return 'never'; },
    openForwardFn: () => { calls.push('openForward'); return { ready: Promise.resolve('/s'), cleanup: () => {} }; },
    requestFn: () => { calls.push('request'); return Promise.resolve({}); },
  }), (e) => e.code === E.SSH_CONTEXT);
  assert.deepEqual(calls, ['context'], 'a bad SSH context must stop before the secret is read');
});

test('REAL header proof: exact GET path and exact header set, token only in x-api-key', () => {
  // The previous version of this test asserted the claim against the injected
  // seam, which observes {token} BEFORE any header is constructed — it could
  // not have failed. This asserts the actual wire options instead.
  const opts = buildRequestOptions({ socketPath: '/s', token: TOKEN_CANARY, timeoutMs: 1000 });
  assert.equal(opts.method, 'GET');
  assert.equal(opts.path, P_PROJECT_ONE + '?projectId=' + QUIZ_PROJECT_ID);
  assert.deepEqual(Object.keys(opts.headers).sort(), ['accept', 'x-api-key']);
  assert.equal(opts.headers['x-api-key'], TOKEN_CANARY);
  assert.ok(!opts.path.includes(TOKEN_CANARY), 'token must never enter the path');
  for (const [k, v] of Object.entries(opts.headers)) {
    if (k === 'x-api-key') continue;
    assert.ok(!String(v).includes(TOKEN_CANARY), 'token leaked into header ' + k);
  }
  assert.ok(!('authorization' in opts.headers) && !('cookie' in opts.headers));
});

test('composed run: the seam args build exactly those wire options', async () => {
  const h = harness({ body: { projectId: QUIZ_PROJECT_ID, organizationId: QUIZ_ORG_ID } });
  await orchestrate(h.args);
  const opts = buildRequestOptions(h.seen[0]);
  assert.equal(opts.socketPath, '/fake/socket');
  assert.equal(opts.path, P_PROJECT_ONE + '?projectId=' + QUIZ_PROJECT_ID);
  assert.deepEqual(Object.keys(opts.headers).sort(), ['accept', 'x-api-key']);
  assert.equal(opts.headers['x-api-key'], TOKEN_CANARY);
});

test('a wrong organization fails and still cleans up the forward', async () => {
  const h = harness({ body: { projectId: QUIZ_PROJECT_ID, organizationId: '390t2CmQcAVKegyimLLd3' } });
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.ORG_MISMATCH);
  assert.equal(h.cleanedCount(), 1, 'cleanup must run on the failure path too');
});

test('a failing forward never issues a request, and still cleans up', async () => {
  const h = harness({ failForward: true });
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.FORWARD_FAILED);
  assert.ok(!h.calls.includes('request'), 'no request may be attempted without a socket');
  assert.equal(h.cleanedCount(), 1);
});

test('NO RETRY: a failing request is not attempted a second time', async () => {
  let attempts = 0;
  await assert.rejects(() => orchestrate({
    assertContextFn: () => true,
    readTokenFn: () => TOKEN_CANARY,
    openForwardFn: () => ({ ready: Promise.resolve('/s'), cleanup: () => {} }),
    requestFn: () => { attempts += 1; return Promise.reject(new Fail(E.NETWORK)); },
  }), (e) => e.code === E.NETWORK);
  assert.equal(attempts, 1, 'exactly one attempt, no retry');
});

test('canary never appears in any failure surface of the composed run', async () => {
  const h = harness({ body: { projectId: 'wrong', organizationId: QUIZ_ORG_ID } });
  const e = await orchestrate(h.args).then(() => null, (err) => err);
  assert.equal(e.code, E.PROJECT_MISMATCH);
  assert.ok(!surface(e).includes(TOKEN_CANARY), 'token canary must not leak');
});

const LATE = { projectId: QUIZ_PROJECT_ID, organizationId: QUIZ_ORG_ID };
test('DEADLINE with an active forward: E_DEADLINE and cleanup exactly once', { timeout: 5000 }, async () => {
  let cleaned = 0;
  let requested = 0;
  await assert.rejects(() => orchestrate({
    deadlineMs: 20,
    assertContextFn: () => true,
    readTokenFn: () => TOKEN_CANARY,
    // forward is UP — this is the case a process.exit timer would strand
    openForwardFn: () => ({ ready: Promise.resolve('/s'), cleanup: () => { cleaned += 1; } }),
    // Settles LATE rather than never: if the deadline is broken this test FAILS
    // cleanly instead of hanging. A hung test is cancelled by the runner and
    // silently drops out of the count, so "fail 0" would stay green on a
    // broken deadline — a gate that disappears is not a gate.
    requestFn: () => { requested += 1; return new Promise((r) => setTimeout(() => r(LATE), 400)); },
  }), (e) => e.code === E.DEADLINE);
  assert.equal(requested, 1);
  assert.equal(cleaned, 1, 'cleanup must run exactly once when the deadline wins');
  await new Promise((r) => setTimeout(r, 40)); // a late loser must not throw
});

test('a deadline that does NOT fire leaves the success path intact', async () => {
  const h = harness({ body: { projectId: QUIZ_PROJECT_ID, organizationId: QUIZ_ORG_ID } });
  assert.equal(await orchestrate({ ...h.args, deadlineMs: 30_000 }), OK_RECEIPT);
  assert.equal(h.cleanedCount(), 1);
});

// ══ Directory ownership policy (C1/C4a) ═════════════════════════════════
const DIR = { isDirectory: () => true, uid: process.getuid(), mode: 0o40700 };
test('owned-dir policy accepts 0700 self-owned, refuses group/other bits and non-dirs', () => {
  assert.equal(classifyOwnedDir(DIR), true);
  assert.equal(classifyOwnedDir({ ...DIR, mode: 0o40750 }), false, 'group-readable refused');
  assert.equal(classifyOwnedDir({ ...DIR, mode: 0o40707 }), false, 'other bits refused');
  assert.equal(classifyOwnedDir({ ...DIR, uid: process.getuid() + 1 }), false, 'foreign uid refused');
  assert.equal(classifyOwnedDir({ ...DIR, isDirectory: () => false }), false, 'symlink/file refused');
  assert.equal(classifyOwnedDir(null), false);
});

// ══ Token file parsing ══════════════════════════════════════════════════
test('accepts exactly one pinned entry', () => {
  assert.equal(parseTokenFile(GOOD), TOKEN_CANARY);
  assert.equal(parseTokenFile(TOKEN_KEY + '=' + TOKEN_CANARY), TOKEN_CANARY);
});

test('rejects BOM, comments, blanks-only, extras, duplicates and malformed', () => {
  const bad = [
    '\ufeff' + GOOD,
    '# comment' + LF,
    LF + LF,
    GOOD + 'OTHER_KEY=x' + LF,
    GOOD + TOKEN_KEY + '=second' + LF,
    'WRONG_KEY=' + TOKEN_CANARY + LF,
    'no-equals-here' + LF,
    '',
  ];
  for (const t of bad) {
    assert.throws(() => parseTokenFile(t), (e) => e instanceof Fail, JSON.stringify(t.slice(0, 24)));
  }
});

test('rejects non-printable, non-ASCII and oversize values', () => {
  for (const v of ['ab\u0000cd', 'caf' + String.fromCodePoint(0xe9), 'a b', 'x'.repeat(5000), '']) {
    assert.throws(() => parseTokenFile(TOKEN_KEY + '=' + v + LF), (e) => e instanceof Fail);
  }
});

test('a failing parse never leaks the value', () => {
  const e = caught(() => parseTokenFile(GOOD + 'EXTRA=x' + LF));
  assert.ok(!surface(e).includes(TOKEN_CANARY));
});

// ══ Response handling ═══════════════════════════════════════════════════
test('status classification', () => {
  for (const st of [401, 403]) assert.throws(() => classifyStatus(st), (e) => e.code === E.AUTH_REJECTED);
  for (const st of [301, 302, 307]) assert.throws(() => classifyStatus(st), (e) => e.code === E.REDIRECT_REFUSED);
  for (const st of [400, 404, 500]) {
    const s = surface(caught(() => classifyStatus(st)));
    assert.ok(s.includes(E.UPSTREAM_STATUS));
    assert.ok(!s.includes(String(st)), 'status must not be reflected');
  }
  assert.equal(classifyStatus(200), true);
});

test('bodies fail closed without echoing content', () => {
  const s = surface(caught(() => parseBounded('{bad ' + TOKEN_CANARY)));
  assert.ok(s.includes(E.BAD_SHAPE));
  assert.ok(!s.includes(TOKEN_CANARY));
  assert.throws(() => parseBounded('"' + 'x'.repeat(1000001) + '"'), (e) => e.code === E.BODY_TOO_LARGE);
});

test('stream aborts above the cap and rejects invalid UTF-8', async () => {
  const flood = new Readable({ read() { this.push(Buffer.alloc(1024, 0x61)); } });
  await assert.rejects(() => consumeBoundedStream(flood, 4096), (e) => e.code === E.BODY_TOO_LARGE);
  await assert.rejects(() => consumeBoundedStream(Readable.from([Buffer.from([0xff, 0xfe])]), 4096),
    (e) => e.code === E.BAD_ENCODING);
});

test('scope assertion requires exact project AND org', () => {
  assert.equal(assertQuizScope({ projectId: QUIZ_PROJECT_ID, organizationId: QUIZ_ORG_ID }), true);
  assert.throws(() => assertQuizScope({ projectId: QUIZ_PROJECT_ID, organizationId: 'x' }), (e) => e.code === E.ORG_MISMATCH);
  assert.throws(() => assertQuizScope({ projectId: 'x', organizationId: QUIZ_ORG_ID }), (e) => e.code === E.PROJECT_MISMATCH);
  for (const b of [null, 'nope', {}]) assert.throws(() => assertQuizScope(b), (e) => e instanceof Fail);
});

// ══ Deadline ════════════════════════════════════════════════════════════
test('deadline beats a synchronous abort callback and settles once', { timeout: 5000 }, async () => {
  let captured = null; let aborts = 0;
  // Self-bounded: a broken deadline strands the promise (the abort path marks
  // it settled, so no late success can rescue it). Racing against our own
  // sentinel turns that stall into a clean FAILURE instead of a cancelled test
  // that silently drops out of the count.
  const outcome = await Promise.race([
    withAbsoluteDeadline({
      ms: 5,
      abort: () => { aborts += 1; if (captured) captured(new Fail(E.NETWORK)); },
      start: (ok, bad) => { captured = bad; },
    }).then(() => 'RESOLVED', (e) => e.code),
    new Promise((res) => setTimeout(() => res('HUNG'), 300)),
  ]);
  assert.equal(outcome, E.DEADLINE, 'deadline must reject with E_DEADLINE, not stall');
  assert.equal(aborts, 1);
});

// ══ Runtime guard ═══════════════════════════════════════════════════════
const CLEAN = { env: {}, execArgv: [], globalAgentIsStock: true };
test('clean runtime accepted; every instrumented state refused', () => {
  assert.equal(classifyRuntime(CLEAN), true);
  const bad = [
    { ...CLEAN, env: { NODE_OPTIONS: '--require ./x' } },
    { ...CLEAN, env: { NODE_DEBUG: 'http' } },
    { ...CLEAN, env: { NODE_DEBUG_NATIVE: '1' } },
    { ...CLEAN, env: { NODE_USE_ENV_PROXY: '1' } },
    { ...CLEAN, execArgv: ['--inspect'] },
    { ...CLEAN, globalAgentIsStock: false },
  ];
  for (const st of bad) assert.throws(() => classifyRuntime(st), (e) => e.code === E.UNSAFE_RUNTIME);
});

test('the live guard refuses under an instrumented runner, proving it is wired', () => {
  assert.throws(() => assertSafeRuntime(), (e) => e.code === E.UNSAFE_RUNTIME);
});

// ══ CLI ═════════════════════════════════════════════════════════════════
test('no-argument action: any argument is refused', async () => {
  for (const a of [['x'], ['--help'], ['probe']]) {
    const r = await run(process.execPath, [SCRIPT, ...a]).catch((e) => e);
    assert.equal(String(r.stdout).trim(), E.BAD_ACTION);
  }
});

// ══ Structural invariants (supplementary to the behavioural gates) ══════
test('no Infisical code, no POST capability, no public endpoint, no globalAgent', () => {
  for (const bad of ['universal-auth', 'secrets/raw', 'accessToken', "'POST'", '167.234.234.41', 'globalAgent)']) {
    assert.ok(!CODE.includes(bad), 'must not contain ' + bad);
  }
  assert.ok(CODE.includes('new http.Agent('), 'uses a private agent');
});

test('SSH options are strict and pinned; no relaxation anywhere', () => {
  for (const o of ['BatchMode=yes', 'StrictHostKeyChecking=yes', 'PasswordAuthentication=no',
                   'KbdInteractiveAuthentication=no', 'PermitLocalCommand=no',
                   'ExitOnForwardFailure=yes', 'IdentitiesOnly=yes']) {
    assert.ok(SSH_ARGS.includes(o), 'missing ' + o);
  }
  for (const relax of ['StrictHostKeyChecking=no', 'accept-new', 'UserKnownHostsFile=/dev/null']) {
    assert.ok(!CODE.includes(relax), 'must never contain ' + relax);
  }
  assert.ok(CODE.includes("SSH_HOST = '100.85.254.44'"), 'host pinned');
  assert.ok(CODE.includes("SSH_USER = 'ubuntu'"), 'user pinned');
});

test('socket readiness uses lstat only — no HTTP probe crosses the forward', () => {
  const fwd = CODE.slice(CODE.indexOf('function openForward'), CODE.indexOf('function requestProjectOne'));
  assert.ok(fwd.includes('lstatSync(sock)'), 'readiness is lstat-based');
  assert.ok(fwd.includes('isSocket()'), 'requires an actual socket');
  assert.ok(!fwd.includes('http.request'), 'no HTTP request may be made during readiness');
});

test('C1: temp dir is mkdtemp under a VERIFIED parent, with no pre-delete', () => {
  const fwd = CODE.slice(CODE.indexOf('function openForward'), CODE.indexOf('function buildRequestOptions'));
  assert.ok(fwd.includes('mkdtempSync(join(TEMP_PARENT'), 'unpredictable dir name');
  assert.ok(fwd.includes('assertOwnedDir(TEMP_PARENT'), 'parent verified before use');
  assert.ok(!fwd.includes('process.pid'), 'no predictable pid-derived path');
  const preDelete = fwd.indexOf('rmSync');
  const create = fwd.indexOf('mkdtempSync');
  assert.ok(preDelete > create, 'nothing may be removed before this run creates it');
});

test('C4a: the fixed token parent is validated before the token is read', () => {
  const rt = CODE.slice(CODE.indexOf('function readToken'), CODE.indexOf('function parseTokenFile'));
  assert.ok(rt.includes('assertOwnedDir(TOKEN_PARENT'), 'parent checked');
  assert.ok(rt.indexOf('assertOwnedDir') < rt.indexOf('openSync'), 'checked BEFORE opening');
});

test('C2: no process.exit anywhere — it would skip the cleanup finally', () => {
  assert.ok(!CODE.includes('process.exit('), 'must use exitCode, never process.exit');
});

// OFFLINE: `ssh -G` resolves effective config and EXITS. It opens no
// connection, authenticates nothing and creates no socket. This is the one
// property the pure and source tests cannot see: whether the composed argv
// actually yields the forward we intend. ClearAllForwardings=yes passed every
// source-level check while silently deleting the -L.
test('C3b: composed argv yields EXACTLY one localforward, to 127.0.0.1:3000', () => {
  const sock = '/tmp/dkscope-effective-config-probe';
  const argv = ['-G', ...SSH_ARGS, '-L', sock + ':127.0.0.1:3000', 'ubuntu@100.85.254.44'];
  const out = execFileSync('/usr/bin/ssh', argv, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  const fwd = out.split('\n').filter((l) => /^localforward /i.test(l.trim()));
  assert.equal(fwd.length, 1, 'expected exactly one localforward, got: ' + JSON.stringify(fwd));
  assert.match(fwd[0].trim(), /^localforward \/tmp\/dkscope-effective-config-probe \[127\.0\.0\.1\]:3000$/);
  const clears = out.split('\n').filter((l) => /^clearallforwardings /i.test(l.trim()));
  assert.deepEqual(clears.map((l) => l.trim()), ['clearallforwardings no'],
    'ClearAllForwardings must resolve to no; yes deletes our own -L');
});

test('C3: ssh reads no config files (-F none)', () => {
  const i = SSH_ARGS.indexOf('-F');
  assert.ok(i >= 0 && SSH_ARGS[i + 1] === 'none', 'must pass -F none');
});

test('temp parent is pwd-derived, never os.tmpdir()', () => {
  assert.ok(!CODE.includes('tmpdir('), 'must not use os.tmpdir(), which follows ambient TMPDIR');
  assert.ok(CODE.includes("TEMP_PARENT = join(HOME, '.config', 'aperture')"));
});

test('no hashing, fingerprinting or length reporting', () => {
  for (const bad of ['createHash', 'sha256', 'byteLength(']) {
    assert.ok(!CODE.includes(bad), 'must not contain ' + bad);
  }
  assert.equal(OK_RECEIPT, 'XEROX_QUIZ_SCOPE_CONFIRMED');
});
