/**
 * Composed synthetic tests for scripts/dokploy-staging-provision.mjs.
 * aperture-ztid5. No live host, no real token, no SSH child, no Dokploy call:
 * every seam is injected and cannot discover a live reader, host or transport.
 *
 * These assert BEHAVIOUR through invocation counts and recorded call order,
 * not source strings. A source-fragment assertion previously passed on the
 * probe while the code violated the very property it claimed to guard.
 *
 * Run: node --test --test-timeout=8000 scripts/__tests__/dokploy-staging-provision.test.mjs
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  orchestrate, projectStagingBinding, projectComposeState, composeMatchesTarget,
  projectDomainRows, domainMatchesTarget, classifyDomainState, buildRequestOptions,
  bodyComposeCreate, bodyComposeSource, bodyComposeEnv, bodyDomainCreate,
  bodyComposeDeploy, extractComposeId, generateStagingEnv, envToBlock,
  assertNoProdIds, E, Fail, OK_RECEIPT, QUIZ_PROJECT_ID, QUIZ_ORG_ID,
  PROD_DENYLIST, STAGING_ENV_NAME, COMPOSE_APPNAME, DOMAIN_HOST, DOMAIN_PORT,
  DOMAIN_SERVICE, INFRA_BRANCH, PINNED_HONO_IMAGE, PINNED_HONO_IMAGE_ID,
  P_PROJECT_ONE, P_COMPOSE_ONE, P_COMPOSE_CREATE, P_COMPOSE_UPDATE,
  P_DOMAIN_BY_COMPOSE, P_DOMAIN_CREATE, P_COMPOSE_DEPLOY,
} from '../dokploy-staging-provision.mjs';

const TOKEN_CANARY = 'CANARY_STAGING_TOKEN_4f81ac02';
const SECRET_CANARY = 'CANARY_ENV_VALUE_9d33be71';
const ENV_ID = 'env_staging_QUIZ_1';
const COMPOSE_ID = 'cmp_staging_QUIZ_1';

function surface(e) {
  let j = ''; try { j = JSON.stringify(e); } catch { j = ''; }
  return [e && e.code, e && e.message, e && e.stack, j].join('|');
}

// project.one body carrying an env field that must NEVER be projected.
function projectBody({ envName = 'staging', composes = [], extraEnvs = [] } = {}) {
  return {
    projectId: QUIZ_PROJECT_ID, organizationId: QUIZ_ORG_ID,
    environments: [
      { environmentId: ENV_ID, name: envName, isDefault: false,
        env: `LEAK=${SECRET_CANARY}`, compose: composes },
      ...extraEnvs,
    ],
  };
}
const PROD_ENV = { environmentId: 'env_prod_1', name: 'production', isDefault: true,
                   env: `LEAK=${SECRET_CANARY}`, compose: [] };

const COMPOSE_OK = {
  composeId: COMPOSE_ID, appName: COMPOSE_APPNAME, sourceType: 'github',
  repository: 'quiz-incluir', owner: 'FranciscoMateusVG', branch: INFRA_BRANCH,
  composePath: './docker-compose.staging.yml', env: `LEAK=${SECRET_CANARY}`,
};
const DOMAIN_OK = {
  domainId: 'dom_1', host: DOMAIN_HOST, port: DOMAIN_PORT, https: true, path: '/',
  serviceName: DOMAIN_SERVICE, certificateType: 'letsencrypt', domainType: 'compose',
  composeId: COMPOSE_ID, enabled: true,
};

// Harness: records every call in order, answers per fixed path.
function harness({ existing = false, domains = [], composeRow = COMPOSE_OK, failAt = null } = {}) {
  const calls = [];
  let cleaned = 0;
  const composes = existing ? [{ composeId: COMPOSE_ID, appName: COMPOSE_APPNAME }] : [];
  const args = {
    assertContextFn: () => { calls.push('context'); return true; },
    readTokenFn: () => { calls.push('readToken'); return TOKEN_CANARY; },
    openForwardFn: () => {
      calls.push('openForward');
      return { ready: Promise.resolve('/fake/sock'), cleanup: () => { cleaned += 1; } };
    },
    envFn: () => ({
      QUIZ_DB_PASSWORD: SECRET_CANARY, SECRET_KEY: SECRET_CANARY,
      ADMIN_USERNAME: 'staging-admin@incluir.test', ADMIN_PASSWORD: SECRET_CANARY,
      STAGING_HONO_IMAGE: PINNED_HONO_IMAGE, STAGING_HONO_DB_PASSWORD: SECRET_CANARY,
      STAGING_BETTER_AUTH_SECRET: SECRET_CANARY,
    }),
    writeCredsFn: () => { calls.push('writeCreds'); return '/fake/creds.env'; },
    requestFn: ({ path, method, body, token }) => {
      const key = path.split('?')[0];
      calls.push(`${method} ${key}`);
      if (failAt === key) return Promise.reject(new Fail(E.UPSTREAM_STATUS));
      if (key === P_PROJECT_ONE) return Promise.resolve(projectBody({ composes }));
      if (key === P_COMPOSE_ONE) return Promise.resolve(composeRow);
      if (key === P_COMPOSE_CREATE) return Promise.resolve({ composeId: COMPOSE_ID });
      if (key === P_DOMAIN_BY_COMPOSE) return Promise.resolve(domains);
      return Promise.resolve({ ok: true, token });
    },
  };
  return { args, calls, cleanedCount: () => cleaned };
}

// ══ Exact sequence and cardinality ══════════════════════════════════════
test('fresh provision: exact ordered sequence, one call per endpoint', async () => {
  const h = harness();
  const r = await orchestrate(h.args);
  assert.equal(r.composeId, COMPOSE_ID);
  assert.equal(r.environmentId, ENV_ID);
  assert.deepEqual(h.calls, [
    'context', 'readToken', 'openForward',
    `GET ${P_PROJECT_ONE}`,
    `POST ${P_COMPOSE_CREATE}`,
    `POST ${P_COMPOSE_UPDATE}`,   // source
    `POST ${P_COMPOSE_UPDATE}`,   // env
    `GET ${P_DOMAIN_BY_COMPOSE}`,
    `POST ${P_DOMAIN_CREATE}`,
    `POST ${P_COMPOSE_DEPLOY}`,
    'writeCreds',
  ]);
  assert.equal(h.cleanedCount(), 1);
});

test('NO MUTATION may precede the identity/environment binding', async () => {
  const h = harness();
  h.args.requestFn = (o) => {
    h.calls.push(`${o.method} ${o.path.split('?')[0]}`);
    return Promise.reject(new Fail(E.PROJECT_MISMATCH));
  };
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.PROJECT_MISMATCH);
  const mutations = h.calls.filter((c) => c.startsWith('POST'));
  assert.deepEqual(mutations, [], 'no POST may be issued before binding succeeds');
});

test('a wrong org fails before any mutation, and still cleans up', async () => {
  const h = harness();
  h.args.requestFn = ({ path, method }) => {
    h.calls.push(`${method} ${path.split('?')[0]}`);
    return Promise.resolve({ projectId: QUIZ_PROJECT_ID, organizationId: 'OTHER_ORG',
                             environments: [] });
  };
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.ORG_MISMATCH);
  assert.deepEqual(h.calls.filter((c) => c.startsWith('POST')), []);
  assert.equal(h.cleanedCount(), 1);
});

// ══ Staging environment selection ═══════════════════════════════════════
test('zero staging environments STOPS — never defaults to production', () => {
  const body = { projectId: QUIZ_PROJECT_ID, organizationId: QUIZ_ORG_ID,
                 environments: [PROD_ENV] };
  assert.throws(() => projectStagingBinding(body), (e) => e.code === E.NO_STAGING_ENV);
});

test('multiple staging environments STOP rather than picking one', () => {
  const body = projectBody({ extraEnvs: [
    { environmentId: 'env_staging_2', name: 'Staging', isDefault: false, compose: [] }] });
  assert.throws(() => projectStagingBinding(body), (e) => e.code === E.MULTI_STAGING_ENV);
});

test('the staging binding never projects environment env values', () => {
  const b = projectStagingBinding(projectBody({ extraEnvs: [PROD_ENV] }));
  assert.equal(b.environmentId, ENV_ID);
  assert.ok(!JSON.stringify(b).includes(SECRET_CANARY), 'env values must not be projected');
  assert.deepEqual(Object.keys(b).sort(), ['environmentCount', 'environmentId', 'existingComposeId']);
});

test('discriminator is the server-declared name, matched case-insensitively', () => {
  assert.equal(STAGING_ENV_NAME, 'staging');
  assert.equal(projectStagingBinding(projectBody({ envName: 'STAGING' })).environmentId, ENV_ID);
});

// ══ Idempotency from authoritative state ════════════════════════════════
test('existing exact compose is REUSED: no create, compose.one verifies it first', async () => {
  const h = harness({ existing: true, domains: [DOMAIN_OK] });
  const r = await orchestrate(h.args);
  assert.equal(r.reusedExisting, true);
  assert.equal(r.domainAction, 'noop');
  assert.ok(!h.calls.includes(`POST ${P_COMPOSE_CREATE}`), 'must not create over an existing service');
  assert.ok(!h.calls.includes(`POST ${P_DOMAIN_CREATE}`), 'exact domain must be a verified no-op');
  assert.equal(h.calls.indexOf(`GET ${P_COMPOSE_ONE}`) < h.calls.findIndex((c) => c.startsWith('POST')),
    true, 'state is read BEFORE any mutation');
});

test('an existing compose pointing elsewhere STOPS, no mutation', async () => {
  const h = harness({ existing: true, composeRow: { ...COMPOSE_OK, branch: 'main' } });
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.COMPOSE_CONFLICT);
  assert.deepEqual(h.calls.filter((c) => c.startsWith('POST')), []);
  assert.equal(h.cleanedCount(), 1);
});

test('conflicting or foreign domain state STOPS — never create-on-unknown', () => {
  assert.equal(classifyDomainState([]), 'create');
  assert.equal(classifyDomainState([DOMAIN_OK]), 'noop');
  for (const rows of [
    [{ ...DOMAIN_OK, port: 8080 }],
    [{ ...DOMAIN_OK, serviceName: 'other' }],
    [{ ...DOMAIN_OK, https: false }],
    [{ ...DOMAIN_OK, certificateType: 'none' }],
    [DOMAIN_OK, { ...DOMAIN_OK, domainId: 'dom_2', host: 'other.example.org' }],
    [{ ...DOMAIN_OK, host: 'other.example.org' }],
  ]) {
    assert.throws(() => classifyDomainState(rows), (e) => e.code === E.DOMAIN_CONFLICT);
  }
});

test('domain projection allowlists structural fields only', () => {
  const [d] = projectDomainRows([{ ...DOMAIN_OK, env: SECRET_CANARY, secretThing: SECRET_CANARY }]);
  assert.ok(!JSON.stringify(d).includes(SECRET_CANARY));
  assert.ok(!('env' in d) && !('secretThing' in d));
});

test('compose projection allowlists structural fields only, never env', () => {
  const st = projectComposeState(COMPOSE_OK);
  assert.ok(!JSON.stringify(st).includes(SECRET_CANARY));
  assert.ok(!('env' in st));
  assert.equal(composeMatchesTarget(st), true);
  assert.equal(composeMatchesTarget({ ...st, owner: 'someone-else' }), false);
});

// ══ Production refusal ══════════════════════════════════════════════════
test('production identifiers are refused structurally', () => {
  assert.ok(PROD_DENYLIST.length > 0);
  for (const id of PROD_DENYLIST) {
    assert.throws(() => assertNoProdIds([id]), (e) => e.code === E.PROD_ID_REFUSED, id);
  }
  assert.throws(() => extractComposeId({ composeId: PROD_DENYLIST[0] }),
    (e) => e.code === E.PROD_ID_REFUSED);
});

test('a prod compose appearing in the project body is refused before mutation', () => {
  const body = projectBody({ composes: [{ composeId: '_A6rI-GEm9oF8ysIojm0O', appName: 'x' }] });
  assert.throws(() => projectStagingBinding(body), (e) => e.code === E.PROD_ID_REFUSED);
});

// ══ First-failure stop ══════════════════════════════════════════════════
test('the sequence stops at the FIRST non-success; later calls never happen', async () => {
  for (const [failPath, expectedAfter] of [
    [P_COMPOSE_CREATE, [P_COMPOSE_UPDATE, P_DOMAIN_CREATE, P_COMPOSE_DEPLOY]],
    [P_COMPOSE_UPDATE, [P_DOMAIN_CREATE, P_COMPOSE_DEPLOY]],
    [P_DOMAIN_CREATE, [P_COMPOSE_DEPLOY]],
  ]) {
    const h = harness({ failAt: failPath });
    await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.UPSTREAM_STATUS);
    for (const later of expectedAfter) {
      assert.ok(!h.calls.includes(`POST ${later}`),
        `${later} must not run after ${failPath} failed`);
    }
    assert.ok(!h.calls.includes('writeCreds'), 'no fixture file on a failed run');
    assert.equal(h.cleanedCount(), 1, 'cleanup runs on the failure path');
  }
});

test('no retry: a failed call is attempted exactly once', async () => {
  let n = 0;
  const h = harness();
  const inner = h.args.requestFn;
  h.args.requestFn = (o) => {
    if (o.path.startsWith(P_COMPOSE_DEPLOY)) { n += 1; return Promise.reject(new Fail(E.NETWORK)); }
    return inner(o);
  };
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.NETWORK);
  assert.equal(n, 1);
});

// ══ Deadline ════════════════════════════════════════════════════════════
test('deadline with an active forward: E_DEADLINE and cleanup exactly once', async () => {
  const h = harness();
  const inner = h.args.requestFn;
  h.args.requestFn = (o) => o.path.startsWith(P_PROJECT_ONE)
    ? new Promise((r) => setTimeout(() => r(projectBody()), 400))  // late, never never
    : inner(o);
  await assert.rejects(() => orchestrate({ ...h.args, deadlineMs: 20 }),
    (e) => e.code === E.DEADLINE);
  assert.equal(h.cleanedCount(), 1);
});

// ══ Wire shape and canary absence ═══════════════════════════════════════
test('token rides only in x-api-key, never in a path or another header', () => {
  const o = buildRequestOptions({ socketPath: '/s', token: TOKEN_CANARY,
                                  path: P_COMPOSE_DEPLOY, method: 'POST', bodyJson: '{}' });
  assert.deepEqual(Object.keys(o.headers).sort(),
    ['accept', 'content-length', 'content-type', 'x-api-key']);
  assert.equal(o.headers['x-api-key'], TOKEN_CANARY);
  assert.ok(!o.path.includes(TOKEN_CANARY));
  for (const [k, v] of Object.entries(o.headers)) {
    if (k !== 'x-api-key') assert.ok(!String(v).includes(TOKEN_CANARY), k);
  }
});

test('fixed bodies contain no secrets and pin the intended target', () => {
  assert.equal(bodyComposeCreate(ENV_ID).environmentId, ENV_ID);
  assert.equal(bodyComposeSource(COMPOSE_ID).branch, INFRA_BRANCH);
  const d = bodyDomainCreate(COMPOSE_ID);
  assert.equal(d.host, DOMAIN_HOST);
  assert.equal(d.port, DOMAIN_PORT);
  assert.equal(d.serviceName, DOMAIN_SERVICE);
  assert.deepEqual(bodyComposeDeploy(COMPOSE_ID), { composeId: COMPOSE_ID });
});

test('generated env pins the image and leaves TRUSTED_PROXY_CIDRS unset', () => {
  const env = generateStagingEnv();
  assert.equal(env.STAGING_HONO_IMAGE, PINNED_HONO_IMAGE);
  assert.ok(!('TRUSTED_PROXY_CIDRS' in env), 'must stay unset: empty is deny-all');
  assert.match(PINNED_HONO_IMAGE_ID, /^sha256:[0-9a-f]{64}$/);
  const block = envToBlock(env);
  assert.equal(block.split('\n').length, Object.keys(env).length, 'one KEY=VALUE per line');
  assert.ok(!block.includes('\\n'), 'literal newlines, not escaped ones');
});

test('an env-block mutation body carries values but never reaches a receipt', async () => {
  const h = harness();
  const r = await orchestrate(h.args);
  const receipt = [OK_RECEIPT, r.composeId, r.environmentId, r.credPath, r.domainAction].join('|');
  assert.ok(!receipt.includes(SECRET_CANARY), 'no secret in the receipt');
  assert.ok(!receipt.includes(TOKEN_CANARY), 'no token in the receipt');
});

test('canaries are absent from every failure surface', async () => {
  for (const failAt of [P_COMPOSE_CREATE, P_DOMAIN_CREATE, P_COMPOSE_DEPLOY]) {
    const h = harness({ failAt });
    const e = await orchestrate(h.args).then(() => null, (err) => err);
    const s = surface(e);
    assert.ok(!s.includes(TOKEN_CANARY), 'token leaked on ' + failAt);
    assert.ok(!s.includes(SECRET_CANARY), 'secret leaked on ' + failAt);
  }
});
