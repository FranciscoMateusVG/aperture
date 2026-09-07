/**
 * Synthetic tests for scripts/dokploy-scope-probe.mjs (aperture-ztid5).
 *
 * ISOLATION: no live host, no real credential, no SSH child spawned, no
 * Dokploy call. The module exports no credential reader, no live transport and
 * no runnable action, so importing it cannot reach anything. Control
 * characters appear as ESCAPES, never literal bytes.
 *
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
  parseCredentials, classifyStatus, parseBounded, consumeBoundedStream,
  withAbsoluteDeadline, makeBudget, charge, extractSecretValue, assertQuizScope,
  assertSafeRuntime, classifyRuntime, SSH_ARGS, E, Fail, OK_RECEIPT,
  QUIZ_PROJECT_ID, QUIZ_ORG_ID, SECRET_NAME, WORKSPACE_ID, ENVIRONMENT,
} from '../dokploy-scope-probe.mjs';

const run = promisify(execFile);
const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(HERE, '..', 'dokploy-scope-probe.mjs');
const SRC = readFileSync(SCRIPT, 'utf8');
const CODE = SRC.split('\n').filter((l) => !l.trim().startsWith('*') && !l.trim().startsWith('//')).join('\n');

const TOKEN_CANARY = 'CANARY_DOKPLOY_TOKEN_5b2e9a71';
const CRED_CANARY = 'CANARY_CLIENT_SECRET_c41f80d3';
const LF = '\u000a';

function surface(e) {
  let j = ''; try { j = JSON.stringify(e); } catch { j = ''; }
  return [e && e.code, e && e.message, e && e.stack, j].join('|');
}

// ══ Module surface ══════════════════════════════════════════════════════
test('exports no credential reader, live transport, or runnable action', async () => {
  const mod = await import('../dokploy-scope-probe.mjs');
  for (const b of ['readCredentials', 'probe', 'request', 'fetchPinnedSecret',
                   'startForward', 'CRED_PATH', 'CRED_DIR', 'main']) {
    assert.ok(!(b in mod), b + ' must not be exported');
  }
});

test('is a NO-ARGUMENT action: any argument is refused', async () => {
  for (const args of [['x'], ['--help'], ['probe'], ['a', 'b']]) {
    const r = await run(process.execPath, [SCRIPT, ...args]).catch((e) => e);
    assert.equal(String(r.stdout).trim(), E.BAD_ACTION);
  }
});

// ══ Pinned constants — the whole point is that these cannot drift ════════
test('Infisical source constants are exactly the approved ones', () => {
  assert.equal(SECRET_NAME, 'DOKPLOY_TOKEN_INCLUIR_XEROX');
  assert.equal(WORKSPACE_ID, 'b4a65c24-dd50-4e93-b323-41d472e7cf46');
  assert.equal(ENVIRONMENT, 'prod');
});

test('Dokploy target constants are exactly the approved ones', () => {
  assert.equal(QUIZ_PROJECT_ID, 'w4FraIVPC0PfP2fZxVtaT');
  assert.equal(QUIZ_ORG_ID, 'GME9CAd599FWcInMNTZ2F');
});

test('single-secret retrieval is pinned: no list, search, fallback or expansion', () => {
  assert.ok(CODE.includes('/api/v3/secrets/raw/'), 'uses the single-secret path');
  assert.ok(CODE.includes("expandSecretReferences: 'false'"));
  assert.ok(CODE.includes("include_imports: 'false'"));
  for (const bad of ['secrets/raw?', 'project.all', 'secretsList', 'search']) {
    assert.ok(!CODE.includes(bad), 'must not contain ' + bad);
  }
});

test('exactly one Dokploy endpoint, read-only, no mutation method reachable', () => {
  assert.ok(CODE.includes('/api/project.one'), 'project.one pinned');
  const methods = [...CODE.matchAll(/method: '([A-Z]+)'/g)].map((m) => m[1]);
  assert.deepEqual([...new Set(methods)].sort(), ['GET', 'POST'],
    'only GET and the Infisical login POST; no PUT/PATCH/DELETE');
  for (const bad of ['project.create', 'project.remove', 'compose.create', 'compose.delete', 'deploy']) {
    assert.ok(!CODE.includes(bad), 'must not contain ' + bad);
  }
});

// ══ SSH context — strict, never relaxed, no public endpoint ══════════════
test('SSH options are strict and cannot be relaxed', () => {
  assert.ok(SSH_ARGS.includes('BatchMode=yes'));
  assert.ok(SSH_ARGS.includes('StrictHostKeyChecking=yes'));
  assert.ok(SSH_ARGS.includes('PasswordAuthentication=no'));
  assert.ok(SSH_ARGS.includes('KbdInteractiveAuthentication=no'));
  assert.ok(SSH_ARGS.includes('PermitLocalCommand=no'));
  assert.ok(SSH_ARGS.includes('ExitOnForwardFailure=yes'));
  for (const relax of ['StrictHostKeyChecking=no', 'StrictHostKeyChecking=accept-new',
                       'UserKnownHostsFile=/dev/null']) {
    assert.ok(!CODE.includes(relax), 'must never contain ' + relax);
  }
});

test('the public plain-HTTP Dokploy endpoint is never referenced', () => {
  assert.ok(!CODE.includes('167.234.234.41'), 'public endpoint must not appear');
  assert.ok(CODE.includes("REMOTE_ADDR = '127.0.0.1:3000'"), 'forward targets the host loopback');
  assert.ok(CODE.includes('/usr/bin/ssh'), 'absolute ssh binary, no PATH lookup');
});

test('the secret never reaches the SSH child argv, env or stdin', () => {
  assert.ok(CODE.includes("env: {}"), 'child gets an empty environment');
  assert.ok(CODE.includes("stdio: ['ignore', 'ignore', 'ignore']"), 'no stdin/stdout channel');
  const spawnCall = CODE.slice(CODE.indexOf('spawn(SSH_BIN'), CODE.indexOf('const cleanup'));
  assert.ok(!spawnCall.includes('token'), 'token must not appear in the spawn call');
});

test('forward cleanup removes the socket directory on every path', () => {
  assert.ok(CODE.includes('rmSync(dir, { recursive: true, force: true })'));
  assert.ok(CODE.includes('fwd.cleanup()'), 'cleanup runs in a finally');
  assert.ok(CODE.includes('finally {' + LF + '    fwd.cleanup();'), 'cleanup is in the finally block');
});

// ══ Secret envelope validation ══════════════════════════════════════════
test('accepts exactly the pinned secret envelope', () => {
  const v = extractSecretValue({ secret: { secretKey: SECRET_NAME, secretValue: TOKEN_CANARY } });
  assert.equal(v, TOKEN_CANARY);
});

test('rejects a wrong key name, wrong shapes, empty and oversize values', () => {
  const bad = [
    {}, { secret: null }, { secret: {} },
    { secret: { secretKey: 'OTHER', secretValue: 'x' } },
    { secret: { secretKey: SECRET_NAME, secretValue: '' } },
    { secret: { secretKey: SECRET_NAME, secretValue: 42 } },
    { secret: { secretKey: SECRET_NAME, secretValue: 'x'.repeat(5000) } },
  ];
  for (const b of bad) assert.throws(() => extractSecretValue(b), (e) => e instanceof Fail);
});

test('rejects control characters inside the secret value', () => {
  assert.throws(
    () => extractSecretValue({ secret: { secretKey: SECRET_NAME, secretValue: 'ab\u0000cd' } }),
    (e) => e.code === E.SECRET_INVALID);
});

test('a FAILING secret extraction never leaks the value', () => {
  const s = surface((() => { try {
    extractSecretValue({ secret: { secretKey: 'WRONG', secretValue: TOKEN_CANARY } });
  } catch (e) { return e; } })());
  assert.ok(!s.includes(TOKEN_CANARY), 'token canary must not appear in the error');
});

// ══ Scope assertion — the actual question being answered ════════════════
test('confirms scope only on an exact project AND org match', () => {
  assert.equal(assertQuizScope({ projectId: QUIZ_PROJECT_ID, organizationId: QUIZ_ORG_ID }), true);
});

test('a different organization is a hard mismatch, not a pass', () => {
  assert.throws(
    () => assertQuizScope({ projectId: QUIZ_PROJECT_ID, organizationId: '390t2CmQcAVKegyimLLd3' }),
    (e) => e.code === E.ORG_MISMATCH);
});

test('a different project is a hard mismatch', () => {
  assert.throws(
    () => assertQuizScope({ projectId: 'someOtherProject', organizationId: QUIZ_ORG_ID }),
    (e) => e.code === E.PROJECT_MISMATCH);
});

test('malformed scope responses fail closed', () => {
  for (const b of [null, 'nope', {}, { projectId: QUIZ_PROJECT_ID }]) {
    assert.throws(() => assertQuizScope(b), (e) => e instanceof Fail);
  }
});

// ══ Response handling ═══════════════════════════════════════════════════
test('401 and 403 map to auth-rejected; redirects refused; other non-200 fails', () => {
  for (const st of [401, 403]) assert.throws(() => classifyStatus(st), (e) => e.code === E.AUTH_REJECTED);
  for (const st of [301, 302, 307]) assert.throws(() => classifyStatus(st), (e) => e.code === E.REDIRECT_REFUSED);
  for (const st of [400, 404, 500]) {
    const s = surface((() => { try { classifyStatus(st); } catch (e) { return e; } })());
    assert.ok(s.includes(E.UPSTREAM_STATUS));
    assert.ok(!s.includes(String(st)), 'status must not be reflected');
  }
  assert.equal(classifyStatus(200), true);
});

test('malformed, oversize and invalid-UTF8 bodies fail closed without echoing', () => {
  const s = surface((() => { try { parseBounded('{bad ' + TOKEN_CANARY); } catch (e) { return e; } })());
  assert.ok(s.includes(E.BAD_SHAPE));
  assert.ok(!s.includes(TOKEN_CANARY), 'body must not be echoed');
  assert.throws(() => parseBounded('"' + 'x'.repeat(1000001) + '"'), (e) => e.code === E.BODY_TOO_LARGE);
});

test('stream aborts above the cap and rejects invalid UTF-8', async () => {
  const flood = new Readable({ read() { this.push(Buffer.alloc(1024, 0x61)); } });
  await assert.rejects(() => consumeBoundedStream(flood, 4096), (e) => e.code === E.BODY_TOO_LARGE);
  await assert.rejects(
    () => consumeBoundedStream(Readable.from([Buffer.from([0xff, 0xfe])]), 4096),
    (e) => e.code === E.BAD_ENCODING);
});

// ══ Deadline ════════════════════════════════════════════════════════════
test('deadline wins over a synchronous abort callback and settles once', async () => {
  let captured = null; let aborts = 0;
  await assert.rejects(
    () => withAbsoluteDeadline({
      ms: 5, abort: () => { aborts += 1; if (captured) captured(new Fail(E.NETWORK)); },
      start: (ok, bad) => { captured = bad; },
    }),
    (e) => e.code === E.DEADLINE);
  assert.equal(aborts, 1);
});

test('charge clamps to remaining action time and rejects at the deadline', () => {
  const b = makeBudget();
  assert.ok(charge(b) > 0);
  const spent = makeBudget(); spent.startedAt = Date.now() - 60000;
  assert.throws(() => charge(spent), (e) => e.code === E.DEADLINE);
});

// ══ Credential parsing (text only; touches no file) ═════════════════════
test('credential parse is strict and never leaks on failure', () => {
  const c = parseCredentials('INFISICAL_CLIENT_ID=a' + LF + 'INFISICAL_CLIENT_SECRET=b' + LF);
  assert.deepEqual(c, { clientId: 'a', clientSecret: 'b' });
  const s = surface((() => { try {
    parseCredentials('INFISICAL_CLIENT_ID=' + CRED_CANARY + LF + 'EXTRA=x' + LF);
  } catch (e) { return e; } })());
  assert.ok(s.includes(E.CRED_PARSE));
  assert.ok(!s.includes(CRED_CANARY), 'credential canary must not appear');
});

// ══ Runtime guard ═══════════════════════════════════════════════════════
const CLEAN = { env: {}, execArgv: [], globalAgentIsStock: true };

test('a clean runtime is accepted', () => {
  assert.equal(classifyRuntime(CLEAN), true);
  assert.equal(classifyRuntime({ ...CLEAN, env: { PATH: '/usr/bin' } }), true);
});

test('every instrumented runtime state is refused', () => {
  const rejected = [
    { ...CLEAN, env: { NODE_OPTIONS: '--require ./x.js' } },
    { ...CLEAN, env: { NODE_OPTIONS: ' ' } },
    { ...CLEAN, env: { NODE_DEBUG: 'http' } },
    { ...CLEAN, env: { NODE_DEBUG_NATIVE: 'http' } },
    { ...CLEAN, env: { NODE_USE_ENV_PROXY: '1' } },
    { ...CLEAN, execArgv: ['--inspect'] },
    { ...CLEAN, execArgv: ['--import', './x.mjs'] },
    { ...CLEAN, globalAgentIsStock: false },
  ];
  for (const st of rejected) {
    assert.throws(() => classifyRuntime(st), (e) => e.code === E.UNSAFE_RUNTIME);
  }
});

test('the live guard refuses under an instrumented runner, which is correct', () => {
  // node --test sets execArgv, so the real guard MUST refuse here. This proves
  // the wiring is live rather than a decorative classifier.
  assert.throws(() => assertSafeRuntime(), (e) => e.code === E.UNSAFE_RUNTIME);
});

// ══ Receipt ═════════════════════════════════════════════════════════════
test('the success receipt is a bare constant with no detail', () => {
  assert.equal(OK_RECEIPT, 'XEROX_QUIZ_SCOPE_CONFIRMED');
  assert.ok(!/[0-9]/.test(OK_RECEIPT.replace(/XEROX|QUIZ|SCOPE|CONFIRMED|_/g, '')),
    'no counts, ids, hashes or lengths in the receipt');
});

test('no hashing, fingerprinting or length reporting anywhere in the source', () => {
  for (const bad of ['createHash', 'sha256', 'byteLength(', '.length)' + ' + ']) {
    assert.ok(!CODE.includes(bad), 'must not contain ' + bad);
  }
});

test('the approved metadata helper is untouched by this module', () => {
  assert.ok(!CODE.includes('infisical-metadata'), 'does not import or modify the approved helper');
});
