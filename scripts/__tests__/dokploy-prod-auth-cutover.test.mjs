import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  Fail, E, OK_PREPARE, OK_PUBLISH, OK_RESTORE,
  QUIZ_PROJECT_ID, QUIZ_ORG_ID, PROD_ENV_ID, COMPOSE_ID, COMPOSE_APPNAME,
  AUTH_KEY, AUTH_VALUE, TRUST_KEY, RELEASE_BRANCH, RELEASE_REV, DOMAIN_ID,
  parseEnvBlock, reconcilePrepareEnv, projectTargetFromProject, projectCompose,
  projectDomain, fullDomainUpdate, assertOldPublishedDomain, assertPreparedDomain,
  serializeState, parseState, assertReleaseRevision, bodyComposeSource,
  requestSpec, buildRequestOptions, classifyRuntime, orchestrate,
} from '../dokploy-prod-auth-cutover.mjs';

const oldEnv = [
  'QUIZ_DB_PASSWORD=fixture_a',
  'SECRET_KEY=fixture_b',
  'ADMIN_USERNAME=quiz-admin',
  'ADMIN_PASSWORD=fixture_c',
  'UNRELATED=value with spaces',
].join('\n') + '\n';

const project = (overrides = {}) => ({
  projectId: QUIZ_PROJECT_ID,
  organizationId: QUIZ_ORG_ID,
  environments: [{
    environmentId: PROD_ENV_ID,
    name: 'production',
    isDefault: true,
    compose: [{ composeId: COMPOSE_ID, appName: COMPOSE_APPNAME }],
  }],
  ...overrides,
});

const compose = (overrides = {}) => ({
  composeId: COMPOSE_ID,
  environmentId: PROD_ENV_ID,
  appName: COMPOSE_APPNAME,
  sourceType: 'github',
  repository: 'quiz-incluir',
  owner: 'FranciscoMateusVG',
  branch: 'main',
  composePath: './docker-compose.prod.yml',
  githubId: 'TOmazYpTr8Wz21abongPE',
  composeType: 'docker-compose',
  autoDeploy: true,
  env: oldEnv,
  ...overrides,
});

const domain = (overrides = {}) => ({
  domainId: DOMAIN_ID,
  composeId: COMPOSE_ID,
  host: 'quiz.programaincluir.org',
  path: '/',
  port: 8080,
  customEntrypoint: null,
  https: true,
  certificateType: 'letsencrypt',
  customCertResolver: null,
  serviceName: 'quiz-incluir-frontend-e17b8a',
  domainType: 'compose',
  internalPath: '/',
  stripPath: false,
  middlewares: [],
  forwardAuthEnabled: false,
  enabled: true,
  ...overrides,
});

const rev = `${RELEASE_REV}\trefs/heads/${RELEASE_BRANCH}\n`;
const forward = () => ({ ready: Promise.resolve('/private/socket'), cleanup() {} });

function makeRequest({ composeBody, domainBody, seen }) {
  return async ({ spec }) => {
    seen.push(structuredClone(spec));
    if (spec.path.startsWith('/api/project.one?')) return project();
    if (spec.path.startsWith('/api/compose.one?')) return composeBody;
    if (spec.path.startsWith('/api/domain.one?')) return domainBody;
    return { success: true };
  };
}

function deps(overrides = {}) {
  return {
    assertContextFn() {}, readTokenFn() { return 'token-canary'; },
    openForwardFn: forward, revisionFn() { return rev; },
    deadlineMs: 1_000, ...overrides,
  };
}

test('prepare env preserves every unrelated line and sets only auth plus deny-all trust', () => {
  const withOld = oldEnv + 'MONOREPO_AUTH_URL=https://app.programaincluir.org\nTRUSTED_PROXY_CIDRS=10.0.1.8/32\n';
  const got = reconcilePrepareEnv(withOld).env;
  assert.equal(got, oldEnv + `${AUTH_KEY}=${AUTH_VALUE}\n${TRUST_KEY}=\n`);
  assert.equal(parseEnvBlock(got).parsed.length, 7);
  assert.match(got, /fixture_a/);
  assert.match(got, /UNRELATED=value with spaces/);
});

test('prepare rejects malformed and duplicate env instead of reconstructing credentials', () => {
  for (const bad of ['', 'A=x\r\n', 'A=x\nA=y\n', 'NO_EQUALS\n', 'A=x\n\n']) {
    assert.throws(() => reconcilePrepareEnv(bad), (e) => e instanceof Fail && e.code === E.ENV_CORRUPT);
  }
});

test('exact production project, compose and domain bindings accept; foreign targets reject', () => {
  assert.equal(projectTargetFromProject(project()), true);
  assert.equal(projectCompose(compose()).branch, 'main');
  assertOldPublishedDomain(projectDomain(domain()));
  assertPreparedDomain(projectDomain(domain({ enabled: false })));
  assert.throws(() => projectTargetFromProject(project({ organizationId: 'foreign' })), /E_ORG_MISMATCH/);
  assert.throws(() => projectCompose(compose({ environmentId: 'staging' })), /E_TARGET_MISMATCH/);
  assert.throws(() => projectDomain(domain({ composeId: 'foreign' })), /E_DOMAIN_MISMATCH/);
});

test('full domain update preserves the complete certificate/path policy while changing fixed routing fields', () => {
  const before = projectDomain(domain({ middlewares: ['m1'], stripPath: true }));
  const off = fullDomainUpdate(before, {
    serviceName: 'quiz-incluir-frontend-e17b8a', port: 8080, enabled: false,
  });
  assert.equal(off.domainId, DOMAIN_ID);
  assert.equal(off.host, before.host);
  assert.equal(off.https, true);
  assert.equal(off.certificateType, 'letsencrypt');
  assert.deepEqual(off.middlewares, ['m1']);
  assert.equal(off.stripPath, true);
  assert.equal(off.enabled, false);
  assert.equal(Object.hasOwn(off, 'composeId'), false);
});

test('state serialization round-trips the supported API env without emitting it in receipts', () => {
  const text = serializeState(compose(), domain());
  const state = parseState(text);
  assert.equal(state.compose.env, oldEnv);
  assert.equal(state.domain.serviceName, 'quiz-incluir-frontend-e17b8a');
  for (const receipt of [OK_PREPARE, OK_PUBLISH, OK_RESTORE]) {
    assert.equal(receipt.includes('fixture_a'), false);
    assert.equal(receipt.includes('bytes'), false);
    assert.equal(receipt.includes('sha'), false);
  }
});

test('release revision gate is exact', () => {
  assert.equal(assertReleaseRevision(rev), true);
  for (const bad of ['', rev + rev, rev.replace(RELEASE_REV, '0'.repeat(40)), rev.replace(RELEASE_BRANCH, 'main')]) {
    assert.throws(() => assertReleaseRevision(bad), /E_RELEASE_REV_MISMATCH/);
  }
});

test('prepare snapshots before first mutation, disables old domain, pins source, preserves env, then deploys', async () => {
  const seen = []; const events = []; let saved = null;
  const receipt = await orchestrate({ phase: 'prepare', ...deps({
    requestFn: async (args) => {
      const spec = args.spec;
      if (spec.method === 'POST') events.push(`post:${spec.path}`);
      return makeRequest({ composeBody: compose(), domainBody: domain(), seen })(args);
    },
    readStateFn() { return null; },
    publishStateFn(text) { events.push('snapshot'); saved = text; },
  }) });
  assert.equal(receipt, OK_PREPARE);
  assert.equal(events[0], 'snapshot');
  assert.deepEqual(events.slice(1), [
    'post:/api/domain.update', 'post:/api/compose.update',
    'post:/api/compose.update', 'post:/api/compose.deploy',
  ]);
  const posts = seen.filter((x) => x.method === 'POST');
  assert.equal(posts[0].body.enabled, false);
  assert.deepEqual(posts[1].body, bodyComposeSource());
  assert.equal(posts[2].body.env, reconcilePrepareEnv(oldEnv).env);
  assert.equal(parseState(saved).compose.env, oldEnv);
});

test('wrong project stops before snapshot and every mutation', async () => {
  const seen = []; let published = false;
  await assert.rejects(orchestrate({ phase: 'prepare', ...deps({
    requestFn: async ({ spec }) => {
      seen.push(spec);
      if (spec.path.startsWith('/api/project.one?')) return project({ organizationId: 'foreign' });
      throw new Error('unreachable');
    },
    readStateFn() { return null; }, publishStateFn() { published = true; },
  }) }), /E_ORG_MISMATCH/);
  assert.equal(published, false);
  assert.equal(seen.some((x) => x.method === 'POST'), false);
});

test('publish requires prepared exact state, retargets once, then deploys once', async () => {
  const seen = []; const state = serializeState(compose(), domain());
  const currentEnv = reconcilePrepareEnv(oldEnv).env;
  const receipt = await orchestrate({ phase: 'publish', ...deps({
    requestFn: makeRequest({
      composeBody: compose({ branch: RELEASE_BRANCH, autoDeploy: false, env: currentEnv }),
      domainBody: domain({ enabled: false }), seen,
    }),
    readStateFn() { return state; }, publishStateFn() { throw new Error('must not publish'); },
  }) });
  assert.equal(receipt, OK_PUBLISH);
  const posts = seen.filter((x) => x.method === 'POST');
  assert.deepEqual(posts.map((x) => x.path), ['/api/domain.update', '/api/compose.deploy']);
  assert.equal(posts[0].body.serviceName, 'quiz-incluir-backend-e17b8a');
  assert.equal(posts[0].body.port, 8000);
  assert.equal(posts[0].body.enabled, true);
});

test('publish safely resumes after domain update if the deploy queue call failed', async () => {
  const seen = []; const state = serializeState(compose(), domain());
  const receipt = await orchestrate({ phase: 'publish', ...deps({
    requestFn: makeRequest({
      composeBody: compose({ branch: RELEASE_BRANCH, autoDeploy: false,
        env: reconcilePrepareEnv(oldEnv).env }),
      domainBody: domain({ serviceName: 'quiz-incluir-backend-e17b8a',
        port: 8000, enabled: true }), seen,
    }),
    readStateFn() { return state; }, publishStateFn() {},
  }) });
  assert.equal(receipt, OK_PUBLISH);
  assert.deepEqual(seen.filter((x) => x.method === 'POST').map((x) => x.path),
    ['/api/compose.deploy']);
});

test('publish refuses before POST when private deployment state is not exact', async () => {
  const cases = [
    { composeBody: compose({ branch: 'main' }), domainBody: domain({ enabled: false }) },
    { composeBody: compose({ branch: RELEASE_BRANCH, autoDeploy: false, env: reconcilePrepareEnv(oldEnv).env }), domainBody: domain({ enabled: true }) },
  ];
  for (const c of cases) {
    const seen = [];
    await assert.rejects(orchestrate({ phase: 'publish', ...deps({
      requestFn: makeRequest({ ...c, seen }),
      readStateFn() { return serializeState(compose(), domain()); },
      publishStateFn() {},
    }) }));
    assert.equal(seen.some((x) => x.method === 'POST'), false);
  }
});

test('restore-metadata writes captured env/source/domain only and never deploys', async () => {
  const seen = []; const state = serializeState(compose(), domain());
  const receipt = await orchestrate({ phase: 'restore-metadata', ...deps({
    revisionFn() { throw new Error('release branch unavailable during rollback'); },
    requestFn: makeRequest({
      composeBody: compose({ branch: RELEASE_BRANCH, autoDeploy: false, env: reconcilePrepareEnv(oldEnv).env }),
      domainBody: domain({ serviceName: 'quiz-incluir-backend-e17b8a', port: 8000, enabled: true }), seen,
    }),
    readStateFn() { return state; }, publishStateFn() {},
  }) });
  assert.equal(receipt, OK_RESTORE);
  const posts = seen.filter((x) => x.method === 'POST');
  assert.deepEqual(posts.map((x) => x.path), [
    '/api/compose.update', '/api/compose.update', '/api/domain.update',
  ]);
  assert.equal(posts[0].body.env, oldEnv);
  assert.equal(posts[1].body.branch, 'main');
  assert.equal(posts[2].body.serviceName, 'quiz-incluir-frontend-e17b8a');
  assert.equal(posts.some((x) => x.path.includes('deploy')), false);
});

test('request options contain token only in x-api-key and use fixed local socket', () => {
  const spec = requestSpec('deploy');
  const opts = buildRequestOptions({ socketPath: '/private/socket', token: 'token-canary', spec,
    bodyJson: JSON.stringify(spec.body) });
  assert.deepEqual(Object.keys(opts.headers).sort(), ['accept', 'content-length', 'content-type', 'x-api-key']);
  assert.equal(opts.headers['x-api-key'], 'token-canary');
  assert.equal(opts.path.includes('token-canary'), false);
  assert.equal(JSON.stringify(spec).includes('token-canary'), false);
});

test('runtime guard accepts clean current Node shape and rejects explicit instrumentation', () => {
  assert.equal(classifyRuntime({ env: {}, execArgv: [], globalAgentIsStock: true }), true);
  for (const state of [
    { env: { NODE_OPTIONS: '--inspect' }, execArgv: [], globalAgentIsStock: true },
    { env: {}, execArgv: ['--import=x'], globalAgentIsStock: true },
    { env: {}, execArgv: [], globalAgentIsStock: false },
  ]) assert.throws(() => classifyRuntime(state), /E_UNSAFE_RUNTIME/);
});

test('module exposes no live credential, state, transport or action seams', async () => {
  const mod = await import('../dokploy-prod-auth-cutover.mjs');
  for (const banned of ['readToken', 'TOKEN_PATH', 'openForward', 'requestOnce', 'main',
    'STATE_PATH', 'readOwnedState', 'publishState', 'readReleaseRevision']) {
    assert.equal(Object.hasOwn(mod, banned), false, banned);
  }
  await assert.rejects(orchestrate({ phase: 'prepare' }), /E_BAD_SHAPE/);
});

test('source has no public Dokploy endpoint, Infisical path, retry, or secret-output primitive', () => {
  const src = readFileSync(new URL('../dokploy-prod-auth-cutover.mjs', import.meta.url), 'utf8');
  for (const banned of ['167.234.234.41', 'secrets/raw', 'universal-auth', 'createHash', 'sha256']) {
    assert.equal(src.includes(banned), false, banned);
  }
  assert.equal(src.includes("'x-api-key'"), true);
  assert.equal(src.includes("'-F', 'none'"), true);
});
