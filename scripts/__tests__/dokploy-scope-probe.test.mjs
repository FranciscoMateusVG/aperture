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
import { execFile } from 'node:child_process';
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

test('the token is passed ONLY as x-api-key, never in a URL or another header', async () => {
  const h = harness({ body: { projectId: QUIZ_PROJECT_ID, organizationId: QUIZ_ORG_ID } });
  await orchestrate(h.args);
  const opts = h.seen[0];
  assert.equal(opts.token, TOKEN_CANARY);
  assert.equal(opts.socketPath, '/fake/socket');
  assert.ok(!JSON.stringify({ p: opts.socketPath }).includes(TOKEN_CANARY));
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
test('deadline beats a synchronous abort callback and settles once', async () => {
  let captured = null; let aborts = 0;
  await assert.rejects(() => withAbsoluteDeadline({
    ms: 5, abort: () => { aborts += 1; if (captured) captured(new Fail(E.NETWORK)); },
    start: (ok, bad) => { captured = bad; },
  }), (e) => e.code === E.DEADLINE);
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
