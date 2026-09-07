import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  Fail, E, OK_RECEIPT, TOKEN_KEY, TRUST_KEY, TRUST_VALUE,
  QUIZ_PROJECT_ID, QUIZ_ORG_ID, STAGING_ENV_ID, COMPOSE_ID, COMPOSE_APPNAME,
  parseTokenFile, classifyRuntime, projectTargetFromProject, projectComposeEnv,
  parseEnvBlock, reconcileProxyEnv, requestSpec, buildRequestOptions,
  orchestrate, SSH_ARGS,
} from '../dokploy-staging-reconcile-proxy.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(HERE, '..', 'dokploy-staging-reconcile-proxy.mjs');
const SRC = readFileSync(SCRIPT, 'utf8');
const TOKEN = 'CANARY_DOKPLOY_TOKEN_d91ae';
const SECRET = 'CANARY_EXISTING_SECRET_0f3ac';
const GOOD_TOKEN = `${TOKEN_KEY}=${TOKEN}\n`;
const BASE_ENV = `QUIZ_DB_PASSWORD=${SECRET}\nSECRET_KEY=abc123\nSTAGING_HONO_IMAGE=sha256:abc`;

const project = (overrides = {}) => ({
  projectId: QUIZ_PROJECT_ID,
  organizationId: QUIZ_ORG_ID,
  environments: [
    { environmentId: 'prod1', name: 'production', isDefault: true,
      compose: [{ composeId: 'prod-compose', appName: 'prod-app' }] },
    { environmentId: STAGING_ENV_ID, name: 'staging', isDefault: false,
      compose: [{ composeId: COMPOSE_ID, appName: COMPOSE_APPNAME }] },
  ],
  ...overrides,
});
const compose = (overrides = {}) => ({
  composeId: COMPOSE_ID, environmentId: STAGING_ENV_ID, appName: COMPOSE_APPNAME,
  sourceType: 'github', repository: 'quiz-incluir', owner: 'FranciscoMateusVG',
  branch: 'aperture-ztid5-staging', composePath: './docker-compose.staging.yml',
  env: BASE_ENV, ...overrides,
});
function surface(err) {
  let json = ''; try { json = JSON.stringify(err); } catch { /* ignore */ }
  return [err?.code, err?.message, err?.stack, json].join('|');
}
function harness({ projectBody = project(), composeBody = compose(), requestFailure = null } = {}) {
  const calls = []; let cleaned = 0;
  return {
    calls, cleaned: () => cleaned,
    args: {
      assertContextFn: () => calls.push({ kind: 'context' }),
      readTokenFn: () => { calls.push({ kind: 'token' }); return TOKEN; },
      openForwardFn: () => ({
        ready: Promise.resolve('/fake/socket'),
        cleanup: () => { cleaned += 1; },
      }),
      requestFn: ({ spec }) => {
        calls.push({ kind: spec.path.includes('project.one') ? 'project'
          : spec.path.includes('compose.one') ? 'compose'
            : spec.path.includes('compose.update') ? 'update' : 'deploy', spec });
        if (requestFailure) return Promise.reject(requestFailure);
        if (spec.path.includes('project.one')) return Promise.resolve(projectBody);
        if (spec.path.includes('compose.one')) return Promise.resolve(composeBody);
        return Promise.resolve({ ok: true });
      },
    },
  };
}

test('preserves every existing env byte and changes only TRUSTED_PROXY_CIDRS', () => {
  const out = reconcileProxyEnv(BASE_ENV);
  assert.equal(out.changed, true);
  assert.equal(out.env, BASE_ENV + `\n${TRUST_KEY}=${TRUST_VALUE}`);
  assert.ok(out.env.includes(SECRET));
  const before = parseEnvBlock(BASE_ENV).parsed;
  const after = parseEnvBlock(out.env).parsed;
  assert.deepEqual(
    after.filter((x) => x.key !== TRUST_KEY).map((x) => x.raw),
    before.map((x) => x.raw),
  );
});

test('replaces only an existing empty or old proxy value and preserves trailing LF', () => {
  for (const old of ['', '192.0.2.1/32']) {
    const raw = BASE_ENV + `\n${TRUST_KEY}=${old}\n`;
    const out = reconcileProxyEnv(raw);
    assert.equal(out.env, BASE_ENV + `\n${TRUST_KEY}=${TRUST_VALUE}\n`);
  }
});

test('already-correct env is byte-identical and classified no-op', () => {
  const raw = BASE_ENV + `\n${TRUST_KEY}=${TRUST_VALUE}`;
  assert.deepEqual(reconcileProxyEnv(raw), { env: raw, changed: false });
});

test('env parser rejects ambiguity, malformed lines and controls without reflection', () => {
  const bad = [
    `A=1\nA=2`, `A=1\n${TRUST_KEY}=x\n${TRUST_KEY}=y`,
    'NO_EQUALS', 'lower=x', 'A=1\n\nB=2', 'A=x\rB=y',
    'A=' + SECRET + '\u0000',
  ];
  for (const raw of bad) {
    const err = (() => { try { reconcileProxyEnv(raw); } catch (e) { return e; } })();
    assert.equal(err.code, E.ENV_CORRUPT);
    assert.ok(!surface(err).includes(SECRET));
  }
});

test('exact project/org/environment/compose/app binding accepts mixed prod sibling', () => {
  assert.equal(projectTargetFromProject(project()), true);
});

test('wrong target variants stop before mutation', () => {
  const variants = [
    project({ projectId: 'wrong' }), project({ organizationId: 'wrong' }),
    project({ environments: [] }),
    project({ environments: [{ environmentId: STAGING_ENV_ID, name: 'production',
      isDefault: false, compose: [{ composeId: COMPOSE_ID, appName: COMPOSE_APPNAME }] }] }),
    project({ environments: [{ environmentId: STAGING_ENV_ID, name: 'staging',
      isDefault: false, compose: [{ composeId: COMPOSE_ID, appName: 'foreign-app' }] }] }),
  ];
  for (const body of variants) assert.throws(() => projectTargetFromProject(body), Fail);
});

test('compose.one binds exact target and returns env only in process', () => {
  assert.equal(projectComposeEnv(compose()), BASE_ENV);
  for (const body of [
    compose({ composeId: 'foreign' }), compose({ environmentId: 'foreign' }),
    compose({ appName: 'foreign' }), compose({ repository: 'foreign' }),
    compose({ branch: 'main' }), compose({ composePath: './docker-compose.yml' }),
    compose({ env: null }),
  ]) assert.throws(() => projectComposeEnv(body), Fail);
});

test('changed state makes exactly two reads, one exact env update, then one deploy', async () => {
  const h = harness();
  assert.equal(await orchestrate(h.args), OK_RECEIPT);
  assert.deepEqual(h.calls.map((c) => c.kind),
    ['context', 'token', 'project', 'compose', 'update', 'deploy']);
  const update = h.calls.find((c) => c.kind === 'update').spec;
  assert.deepEqual(Object.keys(update.body).sort(), ['composeId', 'env']);
  assert.equal(update.body.composeId, COMPOSE_ID);
  assert.equal(update.body.env, BASE_ENV + `\n${TRUST_KEY}=${TRUST_VALUE}`);
  assert.equal(h.cleaned(), 1);
});

test('already-correct target performs zero POSTs', async () => {
  const raw = BASE_ENV + `\n${TRUST_KEY}=${TRUST_VALUE}`;
  const h = harness({ composeBody: compose({ env: raw }) });
  assert.equal(await orchestrate(h.args), OK_RECEIPT);
  assert.deepEqual(h.calls.map((c) => c.kind), ['context', 'token', 'project', 'compose']);
  assert.equal(h.cleaned(), 1);
});

test('wrong binding refuses with zero POSTs and still cleans up', async () => {
  const h = harness({ composeBody: compose({ environmentId: 'foreign' }) });
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.TARGET_MISMATCH);
  assert.deepEqual(h.calls.map((c) => c.kind), ['context', 'token', 'project', 'compose']);
  assert.equal(h.cleaned(), 1);
});

test('first request failure is not retried and cleanup runs', async () => {
  const h = harness({ requestFailure: new Fail(E.NETWORK) });
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.NETWORK);
  assert.equal(h.calls.filter((c) => ['project','compose','update','deploy'].includes(c.kind)).length, 1);
  assert.equal(h.cleaned(), 1);
});

test('token and env canaries never enter receipt or failure surfaces', async () => {
  const h = harness({ composeBody: compose({ env: BASE_ENV + '\nBAD' }) });
  const err = await orchestrate(h.args).then(() => null, (e) => e);
  assert.ok(!surface(err).includes(TOKEN));
  assert.ok(!surface(err).includes(SECRET));
  assert.ok(!OK_RECEIPT.includes(TOKEN) && !OK_RECEIPT.includes(SECRET));
});

test('fixed request table has only approved methods, paths and bodies', () => {
  assert.deepEqual(requestSpec('project'),
    { method: 'GET', path: `/api/project.one?projectId=${QUIZ_PROJECT_ID}` });
  assert.deepEqual(requestSpec('compose'),
    { method: 'GET', path: `/api/compose.one?composeId=${COMPOSE_ID}` });
  assert.deepEqual(requestSpec('deploy'),
    { method: 'POST', path: '/api/compose.deploy', body: { composeId: COMPOSE_ID } });
  assert.throws(() => requestSpec('anything-else'), (e) => e.code === E.INTERNAL);
});

test('wire options keep token only in x-api-key', () => {
  const spec = requestSpec('update', BASE_ENV);
  const bodyJson = JSON.stringify(spec.body);
  const opts = buildRequestOptions({ socketPath: '/s', token: TOKEN, spec, bodyJson });
  assert.deepEqual(Object.keys(opts.headers).sort(),
    ['accept', 'content-length', 'content-type', 'x-api-key']);
  assert.equal(opts.headers['x-api-key'], TOKEN);
  assert.ok(!opts.path.includes(TOKEN));
  assert.ok(!bodyJson.includes(TOKEN));
});

test('token file remains exact, bounded and non-reflecting', () => {
  assert.equal(parseTokenFile(GOOD_TOKEN), TOKEN);
  for (const raw of ['', 'WRONG=x\n', GOOD_TOKEN + 'EXTRA=x\n', `${TOKEN_KEY}=a b\n`]) {
    const err = (() => { try { parseTokenFile(raw); } catch (e) { return e; } })();
    assert.ok(err instanceof Fail);
    assert.ok(!surface(err).includes(TOKEN));
  }
});

test('runtime accepts clean state and rejects explicit instrumentation', () => {
  const clean = { env: {}, execArgv: [], globalAgentIsStock: true };
  assert.equal(classifyRuntime(clean), true);
  for (const state of [
    { ...clean, env: { NODE_OPTIONS: '--inspect' } },
    { ...clean, env: { NODE_DEBUG: 'http' } },
    { ...clean, execArgv: ['--import=x'] },
    { ...clean, globalAgentIsStock: false },
  ]) assert.throws(() => classifyRuntime(state), (e) => e.code === E.UNSAFE_RUNTIME);
});

test('deadline stops an active request and cleanup runs exactly once', { timeout: 2000 }, async () => {
  let cleaned = 0;
  const outcome = orchestrate({
    deadlineMs: 10,
    assertContextFn: () => true,
    readTokenFn: () => TOKEN,
    openForwardFn: () => ({ ready: Promise.resolve('/s'), cleanup: () => { cleaned += 1; } }),
    requestFn: () => new Promise((resolve) => setTimeout(() => resolve(project()), 200)),
  });
  await assert.rejects(() => outcome, (e) => e.code === E.DEADLINE);
  assert.equal(cleaned, 1);
});

test('SSH context remains pinned with no relaxed host verification', () => {
  for (const expected of ['-F', 'none', 'StrictHostKeyChecking=yes', 'IdentitiesOnly=yes',
                          'ExitOnForwardFailure=yes']) assert.ok(SSH_ARGS.includes(expected));
  for (const bad of ['StrictHostKeyChecking=no', 'accept-new', 'UserKnownHostsFile=/dev/null']) {
    assert.ok(!SSH_ARGS.includes(bad));
  }
});

test('operational readers/transports/main remain private', async () => {
  const mod = await import('../dokploy-staging-reconcile-proxy.mjs');
  for (const name of ['readToken', 'openForward', 'requestOnce', 'assertSshContext', 'main',
                      'TOKEN_PATH']) assert.ok(!(name in mod), name + ' escaped');
});

test('no provisioning, generation, fallback or secret-derived receipt surface', () => {
  for (const banned of ['compose.create', 'domain.create', 'environment.create',
                         'randomBytes', 'createHash', 'sha256', 'process.exit(']) {
    assert.ok(!SRC.includes(banned), 'contains ' + banned);
  }
  assert.equal(OK_RECEIPT, 'XEROX_QUIZ_PROXY_RECONCILED');
});
