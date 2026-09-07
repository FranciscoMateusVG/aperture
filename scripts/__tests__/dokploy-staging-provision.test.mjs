/**
 * Composed synthetic tests for scripts/dokploy-staging-provision.mjs.
 * aperture-ztid5. No live host, token, SSH child or Dokploy call: every seam
 * is injected and cannot reach a real reader, host or transport.
 *
 * Assertions are on recorded call ORDER and invocation COUNTS, not source
 * strings -- a source-fragment assertion once passed while the code violated
 * the property it claimed to guard.
 *
 * Run: node --test --test-timeout=8000 scripts/__tests__/dokploy-staging-provision.test.mjs
 */
import { test } from 'node:test';
import * as MOD from '../dokploy-staging-provision.mjs';
import assert from 'node:assert/strict';

import {
  orchestrate, projectStagingBinding, selectTarget, projectComposeState,
  assertComposeIdentity, composeMatchesTarget, projectDomainRows,
  domainMatchesTarget, classifyDomainState, buildRequestOptions,
  bodyComposeCreate, bodyComposeSource, bodyDomainCreate, bodyComposeDeploy,
  extractComposeId, generateStagingEnv, envToBlock, parseEscrowBlock, ESCROW_KEYS,
  escrowDecision, assertAdoptable, isUntouchedStub, assertId, assertInfraRevision,
  intendedAppName, appNameCarriesNonce,
  assertNoProdIds, E, Fail, OK_RECEIPT, QUIZ_PROJECT_ID, QUIZ_ORG_ID,
  PROD_DENYLIST, COMPOSE_APPNAME, DOMAIN_HOST, DOMAIN_PORT, DOMAIN_SERVICE,
  INFRA_BRANCH, PINNED_HONO_IMAGE_ID, P_PROJECT_ONE, P_COMPOSE_ONE,
  P_COMPOSE_CREATE, P_COMPOSE_UPDATE, P_DOMAIN_BY_COMPOSE, P_DOMAIN_CREATE,
  P_COMPOSE_DEPLOY,
} from '../dokploy-staging-provision.mjs';

const TOKEN_CANARY = 'CANARY_STAGING_TOKEN_4f81ac02';
const SECRET_CANARY = 'CANARY_ENV_VALUE_9d33be71';
const ENV_ID = 'env_staging_QUIZ_1';
const COMPOSE_ID = 'cmp_staging_QUIZ_1';
const NONCE = 'a1b2c3d4e5f60718';
const APPNAME_SUFFIXED = intendedAppName(NONCE) + '-ab12cd';
const GOOD_LSREMOTE = '26d6a9eb25b8653356d1aae4659382a62d479c41\trefs/heads/aperture-ztid5-staging\n';

const surface = (e) => {
  let j = ''; try { j = JSON.stringify(e); } catch { j = ''; }
  return [e && e.code, e && e.message, e && e.stack, j].join('|');
};

const ESCROW_ENV = Object.freeze({
  QUIZ_DB_PASSWORD: SECRET_CANARY, SECRET_KEY: SECRET_CANARY,
  ADMIN_USERNAME: 'staging-admin@incluir.test', ADMIN_PASSWORD: SECRET_CANARY,
  STAGING_HONO_IMAGE: PINNED_HONO_IMAGE_ID, STAGING_HONO_DB_PASSWORD: SECRET_CANARY,
  STAGING_BETTER_AUTH_SECRET: SECRET_CANARY,
});

function projectBody({ envName = 'staging', composes = [], extraEnvs = [], envOverride } = {}) {
  const e = { environmentId: ENV_ID, name: envName, isDefault: false,
              env: `LEAK=${SECRET_CANARY}`, compose: composes };
  if (envOverride) Object.assign(e, envOverride);
  return { projectId: QUIZ_PROJECT_ID, organizationId: QUIZ_ORG_ID,
           environments: [e, ...extraEnvs] };
}
const PROD_ENV = { environmentId: 'env_prod_1', name: 'production', isDefault: true,
                   env: `LEAK=${SECRET_CANARY}`, compose: [] };
const COMPOSE_ROW = {
  composeId: COMPOSE_ID, appName: APPNAME_SUFFIXED, environmentId: ENV_ID,
  sourceType: 'github', repository: 'quiz-incluir', owner: 'FranciscoMateusVG',
  branch: INFRA_BRANCH, composePath: './docker-compose.staging.yml',
  env: `LEAK=${SECRET_CANARY}`,
};
const DOMAIN_ROW = {
  domainId: 'dom_1', host: DOMAIN_HOST, port: DOMAIN_PORT, https: true, path: '/',
  serviceName: DOMAIN_SERVICE, certificateType: 'letsencrypt', domainType: 'compose',
  composeId: COMPOSE_ID, enabled: true,
};

function harness({ prior = null, intent = { nonce: NONCE }, existingRows = [], domains = [],
                   composeRow = COMPOSE_ROW, failAt = null, lsRemote = GOOD_LSREMOTE } = {}) {
  const calls = [];
  const escrowReads = [];
  const savedBindings = [];
  let cleaned = 0;
  const args = {
    assertContextFn: () => { calls.push('context'); return true; },
    readTokenFn: () => { calls.push('readToken'); return TOKEN_CANARY; },
    escrowFn: () => { escrowReads.push('escrow'); return { env: { ...ESCROW_ENV }, reused: prior !== null }; },
    loadBindingFn: () => prior,
    loadIntentFn: () => intent,
    createIntentFn: () => { calls.push('createIntent'); return { nonce: NONCE }; },
    infraRevFn: () => lsRemote,
    saveBindingFn: (id) => { calls.push('saveBinding'); savedBindings.push(id); return '/fake/b'; },
    openForwardFn: () => {
      calls.push('openForward');
      return { ready: Promise.resolve('/fake/sock'), cleanup: () => { cleaned += 1; } };
    },
    requestFn: ({ path, method, token }) => {
      const key = path.split('?')[0];
      calls.push(`${method} ${key}`);
      if (failAt === key) return Promise.reject(new Fail(E.UPSTREAM_STATUS));
      if (key === P_PROJECT_ONE) return Promise.resolve(projectBody({ composes: existingRows }));
      if (key === P_COMPOSE_ONE) return Promise.resolve(composeRow);
      if (key === P_COMPOSE_CREATE) return Promise.resolve({ composeId: COMPOSE_ID });
      if (key === P_DOMAIN_BY_COMPOSE) return Promise.resolve(domains);
      return Promise.resolve({ ok: true, token });
    },
  };
  return { args, calls, savedBindings, escrowReads, cleanedCount: () => cleaned,
           posts: () => calls.filter((c) => c.startsWith('POST')) };
}
const EXISTING = [{ composeId: COMPOSE_ID, appName: APPNAME_SUFFIXED }];

// ══ Sequence: every read precedes the mutations ═════════════════════════
test('fresh provision: create, rebind, read domain, THEN mutate', async () => {
  const h = harness({ intent: null });   // no prior intent: one is minted first
  const r = await orchestrate(h.args);
  assert.deepEqual(h.calls, [
    'context', 'readToken', 'openForward',
    `GET ${P_PROJECT_ONE}`,
    'createIntent', `POST ${P_COMPOSE_CREATE}`, 'saveBinding',
    `GET ${P_COMPOSE_ONE}`, `GET ${P_DOMAIN_BY_COMPOSE}`,
    `POST ${P_COMPOSE_UPDATE}`, `POST ${P_COMPOSE_UPDATE}`,
    `POST ${P_DOMAIN_CREATE}`, `POST ${P_COMPOSE_DEPLOY}`,
  ]);
  assert.equal(r.composeId, COMPOSE_ID);
  assert.equal(h.cleanedCount(), 1);
});

test('the binding is saved BEFORE any further mutation, so a crash can rebind', async () => {
  const h = harness({ intent: null });
  await orchestrate(h.args);
  assert.deepEqual(h.savedBindings, [COMPOSE_ID]);
  assert.ok(h.calls.indexOf('saveBinding') < h.calls.indexOf(`POST ${P_COMPOSE_UPDATE}`));
});

test('existing service: reads happen with ZERO prior POSTs', async () => {
  const h = harness({ prior: { composeId: COMPOSE_ID }, existingRows: EXISTING,
                      domains: [DOMAIN_ROW] });
  const r = await orchestrate(h.args);
  assert.equal(r.reusedExisting, true);
  assert.equal(r.domainAction, 'noop');
  assert.ok(!h.calls.includes(`POST ${P_COMPOSE_CREATE}`));
  assert.ok(!h.calls.includes(`POST ${P_DOMAIN_CREATE}`), 'exact enabled domain is a no-op');
  const firstPost = h.calls.findIndex((c) => c.startsWith('POST'));
  assert.ok(h.calls.indexOf(`GET ${P_COMPOSE_ONE}`) < firstPost);
  assert.ok(h.calls.indexOf(`GET ${P_DOMAIN_BY_COMPOSE}`) < firstPost);
});

// Cipher's named regression #1
test('REGRESSION: an existing domain conflict produces ZERO POSTs', async () => {
  const conflicting = [
    [{ ...DOMAIN_ROW, port: 8080 }],
    [{ ...DOMAIN_ROW, enabled: false }],
    [DOMAIN_ROW, { ...DOMAIN_ROW, domainId: 'dom_2' }],
  ];
  for (const domains of conflicting) {
    const h = harness({ prior: { composeId: COMPOSE_ID }, existingRows: EXISTING, domains });
    await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.DOMAIN_CONFLICT);
    assert.deepEqual(h.posts(), [], 'no mutation may precede the domain conflict');
    assert.equal(h.cleanedCount(), 1);
  }
});

// Cipher's named regression #2
test('REGRESSION: every partial-failure resume reuses the SAME escrowed secrets', async () => {
  const seen = [];
  for (const failAt of [P_COMPOSE_CREATE, P_COMPOSE_ONE, P_DOMAIN_BY_COMPOSE,
                        P_COMPOSE_UPDATE, P_DOMAIN_CREATE, P_COMPOSE_DEPLOY]) {
    const h = harness({ failAt });
    const captured = [];
    const inner = h.args.requestFn;
    h.args.requestFn = (o) => {
      if (o.body && typeof o.body.env === 'string') captured.push(o.body.env);
      return inner(o);
    };
    await orchestrate(h.args).catch(() => {});
    seen.push(...captured);
  }
  // A successful run afterwards must send the identical env block.
  const ok = harness();
  const sent = [];
  const inner = ok.args.requestFn;
  ok.args.requestFn = (o) => {
    if (o.body && typeof o.body.env === 'string') sent.push(o.body.env);
    return inner(o);
  };
  await orchestrate(ok.args);
  const expected = envToBlock(ESCROW_ENV);
  for (const block of [...seen, ...sent]) {
    assert.equal(block, expected, 'secrets must never be regenerated between attempts');
  }
});

test('REUSE: an existing escrow is authoritative — nothing is regenerated', () => {
  const stored = envToBlock(ESCROW_ENV) + '\n';
  let published = 0;
  const r = escrowDecision({ existingText: stored, generateFn: generateStagingEnv,
                             publishFn: () => { published += 1; return stored; } });
  assert.equal(r.reused, true);
  assert.equal(published, 0, 'an existing escrow must never be rewritten');
  for (const k of ESCROW_KEYS) assert.equal(r.env[k], ESCROW_ENV[k], k);
});

test('CREATE: with no escrow, one publish happens and only what LANDED is used', () => {
  let stored = null;
  const published = [];
  const r = escrowDecision({
    existingText: null, generateFn: generateStagingEnv,
    publishFn: (t) => { published.push(t); stored = t; return t; },
  });
  assert.equal(r.reused, false);
  assert.equal(published.length, 1);
  assert.equal(r.env.STAGING_HONO_IMAGE, PINNED_HONO_IMAGE_ID);
  const again = escrowDecision({ existingText: stored, generateFn: () => {
    throw new Error('regenerated after the escrow existed');
  }, publishFn: () => { throw new Error('republished'); } });
  assert.equal(again.reused, true);
  assert.equal(again.env.SECRET_KEY, r.env.SECRET_KEY);
});

test('a publish that does not land fails closed', () => {
  assert.throws(() => escrowDecision({ existingText: null, generateFn: generateStagingEnv,
    publishFn: () => null }), (e) => e.code === E.ESCROW_WRITE);
  assert.throws(() => escrowDecision({ existingText: null }), (e) => e.code === E.BAD_SHAPE);
});

// ── Cipher F1: no live secret reader may escape the module ──────────────
test('MODULE SURFACE: no export can discover or return the live escrow', () => {
  const banned = ['loadOrCreateEscrow', 'readToken', 'openForward', 'requestOnce', 'main',
                  'TOKEN_PATH', 'TOKEN_PARENT', 'ESCROW_PATH', 'BINDING_PATH',
                  'readOwnedFile', 'publishFile', 'loadBinding', 'saveBinding',
                  'defaultEscrowRead', 'defaultEscrowPublish'];
  for (const b of banned) assert.ok(!(b in MOD), 'must not export ' + b);
  // The exported decision seam has NO defaults: with nothing injected it cannot
  // fall back to a live reader, it refuses.
  assert.throws(() => escrowDecision({}), (e) => e.code === E.BAD_SHAPE);
  assert.throws(() => escrowDecision({ existingText: null, generateFn: generateStagingEnv }),
    (e) => e.code === E.BAD_SHAPE);
});

test('MODULE SURFACE: orchestrate refuses to run without injected seams', async () => {
  await assert.rejects(() => orchestrate({}), (e) => e.code === E.BAD_SHAPE);
  const h = harness();
  for (const missing of ['escrowFn', 'loadBindingFn', 'saveBindingFn', 'requestFn']) {
    const args = { ...h.args };
    delete args[missing];
    await assert.rejects(() => orchestrate(args), (e) => e.code === E.BAD_SHAPE, missing);
  }
});

// ── Cipher F2: escrow and binding must be coupled ───────────────────────
test('binding present + escrow missing STOPS before the forward and any POST', async () => {
  const h = harness({ prior: { composeId: COMPOSE_ID }, existingRows: EXISTING });
  let forwards = 0;
  h.args.escrowFn = () => ({ env: { ...ESCROW_ENV }, reused: false });  // freshly generated
  h.args.openForwardFn = () => { forwards += 1; return { ready: Promise.resolve('/s'), cleanup: () => {} }; };
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.ESCROW_MISSING);
  assert.equal(forwards, 0, 'must stop before the forward opens');
  assert.deepEqual(h.posts(), []);
});

test('adopting an orphan with a fresh escrow also STOPS', async () => {
  const h = harness({ existingRows: EXISTING });
  h.args.escrowFn = () => ({ env: { ...ESCROW_ENV }, reused: false });
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.ESCROW_MISSING);
  assert.deepEqual(h.posts(), [], 'no mutation on a lost-escrow adoption');
});

// ── Cipher F3: the create-crash window is recoverable ───────────────────
test('RECOVERY: create succeeds, binding write fails, second run adopts it', async () => {
  // Run 1: compose.create lands remotely, then saveBinding throws.
  const h1 = harness();
  h1.args.saveBindingFn = () => { throw new Fail(E.ESCROW_WRITE); };
  await assert.rejects(() => orchestrate(h1.args), (e) => e.code === E.ESCROW_WRITE);
  assert.equal(h1.calls.filter((c) => c === `POST ${P_COMPOSE_CREATE}`).length, 1);

  // Run 2: no local binding, one suffixed orphan present, escrow REUSED.
  const h2 = harness({ existingRows: EXISTING, prior: null,
                       composeRow: { ...COMPOSE_ROW, repository: null, owner: null, branch: null } });
  h2.args.escrowFn = () => ({ env: { ...ESCROW_ENV }, reused: true });
  const sent = [];
  const inner = h2.args.requestFn;
  h2.args.requestFn = (o) => { if (o.body?.env) sent.push(o.body.env); return inner(o); };
  const r = await orchestrate(h2.args);
  assert.equal(r.composeId, COMPOSE_ID, 'adopts the orphan');
  assert.ok(!h2.calls.includes(`POST ${P_COMPOSE_CREATE}`), 'NO duplicate create');
  assert.deepEqual(h2.savedBindings, [COMPOSE_ID], 'binding persisted on adoption');
  assert.equal(sent[0], envToBlock(ESCROW_ENV), 'identical escrow reused');
  assert.ok(h2.calls.indexOf('saveBinding') < h2.calls.findIndex(
    (c) => c === `POST ${P_COMPOSE_UPDATE}`), 'binding persisted before mutating');
});

test('adoption is refused when the candidate is someone else shape, or ambiguous', () => {
  // The stub shape now REQUIRES the documented sourceType and composePath too.
  assert.equal(assertAdoptable({ sourceType: 'github',
    composePath: './docker-compose.staging.yml',
    repository: null, owner: null, branch: null }), true);
  assert.equal(assertAdoptable(projectComposeState(COMPOSE_ROW)), true);
  assert.throws(() => assertAdoptable({ repository: 'other', owner: 'x', branch: 'main',
    sourceType: 'github', composePath: './x.yml' }), (e) => e.code === E.COMPOSE_CONFLICT);
  const two = [{ composeId: 'a', appName: COMPOSE_APPNAME + '-1' },
               { composeId: 'b', appName: COMPOSE_APPNAME + '-2' }];
  const b = projectStagingBinding(projectBody({ composes: two }));
  assert.throws(() => selectTarget(b, null, { nonce: NONCE }), (e) => e.code === E.ADOPT_AMBIGUOUS);
});

// ── Cipher F4: strict escrow grammar ────────────────────────────────────
test('duplicate keys are REJECTED, not silently overwritten', () => {
  const good = envToBlock(ESCROW_ENV) + '\n';
  assert.equal(parseEscrowBlock(good).SECRET_KEY, SECRET_CANARY);
  // The exact probe Cipher ran: a second SECRET_KEY previously replaced the first.
  // NOTE: an APPENDED line also trips the line-count check, so it would pass for
  // the wrong reason and could not detect a missing duplicate guard. This block
  // has the CORRECT line count and substitutes a duplicate for another key, so
  // only the duplicate check can reject it.
  const dup = ESCROW_KEYS.map((k) => (k === 'ADMIN_USERNAME'
    ? 'SECRET_KEY=ATTACKER_VALUE' : `${k}=${ESCROW_ENV[k]}`)).join('\n') + '\n';
  assert.equal(dup.split('\n').length - 1, ESCROW_KEYS.length, 'line count is correct');
  assert.throws(() => parseEscrowBlock(dup), (e) => e.code === E.ESCROW_CORRUPT);
  // And the appended form must still be rejected.
  assert.throws(() => parseEscrowBlock(good.trimEnd() + '\nSECRET_KEY=X\n'),
    (e) => e.code === E.ESCROW_CORRUPT);
});

test('values are bounded and control-free; the block is exactly the fixed keys', () => {
  const mk = (over) => envToBlock({ ...ESCROW_ENV, ...over }) + '\n';
  assert.throws(() => parseEscrowBlock(mk({ SECRET_KEY: 'x'.repeat(600) })), (e) => e.code === E.ESCROW_CORRUPT);
  assert.throws(() => parseEscrowBlock(mk({ SECRET_KEY: 'a\u0000b' })), (e) => e.code === E.ESCROW_CORRUPT);
  assert.throws(() => parseEscrowBlock(mk({ SECRET_KEY: '' })), (e) => e.code === E.ESCROW_CORRUPT);
  assert.throws(() => parseEscrowBlock(envToBlock(ESCROW_ENV)), (e) => e.code === E.ESCROW_CORRUPT); // no trailing LF
  assert.throws(() => parseEscrowBlock(good_extra()), (e) => e.code === E.ESCROW_CORRUPT);
  function good_extra() { return envToBlock(ESCROW_ENV) + '\nEXTRA=1\n'; }
});

test('secrets are escrowed BEFORE the forward opens, never after a side effect', async () => {
  const order = [];
  const h = harness();
  h.args.escrowFn = () => { order.push('escrow'); return { env: { ...ESCROW_ENV }, reused: false }; };
  h.args.openForwardFn = () => {
    order.push('forward');
    return { ready: Promise.resolve('/s'), cleanup: () => {} };
  };
  await orchestrate(h.args);
  assert.deepEqual(order, ['escrow', 'forward']);
});

// ══ Strict gate-1 projection ════════════════════════════════════════════
test('unknown compose state is a HARD STOP, never treated as absent', () => {
  for (const override of [{ compose: undefined }, { compose: null }, { compose: {} },
                          { compose: 'nope' }]) {
    assert.throws(() => projectStagingBinding(projectBody({ envOverride: override })),
      (e) => e.code === E.BAD_SHAPE, JSON.stringify(override));
  }
});

test('malformed compose rows stop instead of falling through to create', () => {
  for (const row of [{ composeId: '', appName: 'x' }, { composeId: 'a', appName: '' },
                     { composeId: null, appName: 'x' }, { appName: 'x' }, {}, null]) {
    assert.throws(() => projectStagingBinding(projectBody({ composes: [row] })),
      (e) => e.code === E.BAD_SHAPE);
  }
});

test('zero/multiple staging environments stop; production is never a fallback', () => {
  assert.throws(() => projectStagingBinding({ projectId: QUIZ_PROJECT_ID,
    organizationId: QUIZ_ORG_ID, environments: [PROD_ENV] }), (e) => e.code === E.NO_STAGING_ENV);
  assert.throws(() => projectStagingBinding(projectBody({ extraEnvs: [
    { environmentId: 'env_s2', name: 'Staging', compose: [] }] })),
    (e) => e.code === E.MULTI_STAGING_ENV);
});

test('binding never projects environment env values', () => {
  const b = projectStagingBinding(projectBody({ extraEnvs: [PROD_ENV] }));
  assert.ok(!JSON.stringify(b).includes(SECRET_CANARY));
});

// ══ Target selection: no prefix acceptance, no first-of-many ════════════
test('a single prefix collision is ADOPTED, never duplicated; two are ambiguous', () => {
  // One candidate is the crash-orphan case: adopt it rather than create a
  // second service, but only after compose.one proves its shape (asserted in
  // the recovery test). More than one is unresolvable.
  const b = projectStagingBinding(projectBody({ composes: EXISTING }));
  assert.deepEqual(selectTarget(b, null, { nonce: NONCE }), { composeId: COMPOSE_ID, create: false, adopt: true });
  const two = projectStagingBinding(projectBody({ composes: [
    { composeId: 'a', appName: COMPOSE_APPNAME }, { composeId: 'b', appName: COMPOSE_APPNAME + '-2' }] }));
  assert.throws(() => selectTarget(two, null, { nonce: NONCE }), (e) => e.code === E.ADOPT_AMBIGUOUS);
});

test('two rows matching a prior binding stop; exactly one is required', () => {
  const dup = [{ composeId: COMPOSE_ID, appName: APPNAME_SUFFIXED },
               { composeId: COMPOSE_ID, appName: COMPOSE_APPNAME + '-zz99' }];
  const b = projectStagingBinding(projectBody({ composes: dup }));
  assert.throws(() => selectTarget(b, { composeId: COMPOSE_ID }), (e) => e.code === E.COMPOSE_CONFLICT);
});

test('a prior binding that no longer exists stops, never silently recreates', () => {
  const b = projectStagingBinding(projectBody({ composes: [] }));
  assert.throws(() => selectTarget(b, { composeId: 'gone' }, null), (e) => e.code === E.COMPOSE_CONFLICT);
  assert.deepEqual(selectTarget(b, null, null), { composeId: null, create: true, adopt: false });
});

// ══ Ownership binding ═══════════════════════════════════════════════════
test('identity binding requires exact composeId, environment and appName', () => {
  const st = projectComposeState(COMPOSE_ROW);
  assert.equal(assertComposeIdentity(st, COMPOSE_ID, ENV_ID), true);
  assert.throws(() => assertComposeIdentity(st, 'other', ENV_ID), (e) => e.code === E.IDENTITY_UNPROVEN);
  assert.throws(() => assertComposeIdentity(st, COMPOSE_ID, 'env_prod_1'), (e) => e.code === E.IDENTITY_UNPROVEN);
  assert.throws(() => assertComposeIdentity({ ...st, appName: 'unrelated' }, COMPOSE_ID, ENV_ID),
    (e) => e.code === E.IDENTITY_UNPROVEN);
});

test('a create returning a foreign id is refused before it is mutated', async () => {
  const h = harness();
  h.args.requestFn = ({ path, method }) => {
    const key = path.split('?')[0];
    h.calls.push(`${method} ${key}`);
    if (key === P_PROJECT_ONE) return Promise.resolve(projectBody());
    if (key === P_COMPOSE_CREATE) return Promise.resolve({ composeId: 'not-ours' });
    // The row exists but lives in ANOTHER environment: a returned id that is
    // merely absent from the denylist is not proof of ownership.
    if (key === P_COMPOSE_ONE) {
      return Promise.resolve({ ...COMPOSE_ROW, composeId: 'not-ours',
                               environmentId: 'env_prod_1' });
    }
    return Promise.resolve([]);
  };
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.IDENTITY_UNPROVEN);
  assert.ok(!h.calls.includes(`POST ${P_COMPOSE_UPDATE}`), 'a foreign row must never be updated');
});

test('domain rows bound to another compose are refused', () => {
  assert.throws(() => classifyDomainState([{ ...DOMAIN_ROW, composeId: 'other' }], COMPOSE_ID),
    (e) => e.code === E.IDENTITY_UNPROVEN);
  assert.throws(() => classifyDomainState([DOMAIN_ROW], ''), (e) => e.code === E.IDENTITY_UNPROVEN);
});

test('a DISABLED domain is not a no-op', () => {
  assert.equal(classifyDomainState([DOMAIN_ROW], COMPOSE_ID), 'noop');
  assert.throws(() => classifyDomainState([{ ...DOMAIN_ROW, enabled: false }], COMPOSE_ID),
    (e) => e.code === E.DOMAIN_CONFLICT);
  assert.equal(classifyDomainState([], COMPOSE_ID), 'create');
});

test('an existing compose pointing elsewhere stops with zero mutations', async () => {
  const h = harness({ prior: { composeId: COMPOSE_ID }, existingRows: EXISTING,
                      composeRow: { ...COMPOSE_ROW, branch: 'main' } });
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.COMPOSE_CONFLICT);
  assert.deepEqual(h.posts(), []);
});

// ══ Image pin is ENFORCED, not documentary ══════════════════════════════
test('the env sends the immutable image ID, and a tag-only value is refused', async () => {
  assert.match(PINNED_HONO_IMAGE_ID, /^sha256:[0-9a-f]{64}$/);
  assert.equal(generateStagingEnv().STAGING_HONO_IMAGE, PINNED_HONO_IMAGE_ID);
  const h = harness();
  h.args.escrowFn = () => ({ env: { ...ESCROW_ENV, STAGING_HONO_IMAGE: 'quiz-staging-hono:9cb605fc' },
                             reused: false });
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.IMAGE_NOT_PINNED);
  assert.deepEqual(h.posts(), [], 'an unpinned image must stop before any mutation');
});

test('escrow parsing rejects corruption, drift and a re-introduced proxy CIDR', () => {
  const good = envToBlock(ESCROW_ENV) + '\n';
  assert.equal(parseEscrowBlock(good).SECRET_KEY, SECRET_CANARY);
  assert.throws(() => parseEscrowBlock(good + 'TRUSTED_PROXY_CIDRS=10.0.1.0/24\n'),
    (e) => e.code === E.ESCROW_CORRUPT);
  assert.throws(() => parseEscrowBlock('QUIZ_DB_PASSWORD=x\n'), (e) => e.code === E.ESCROW_CORRUPT);
  assert.throws(() => parseEscrowBlock('novalue\n'), (e) => e.code === E.ESCROW_CORRUPT);
  assert.throws(() => parseEscrowBlock(envToBlock({ ...ESCROW_ENV,
    STAGING_HONO_IMAGE: 'quiz-staging-hono:9cb605fc' }) + '\n'), (e) => e.code === E.IMAGE_NOT_PINNED);
  assert.equal(ESCROW_KEYS.includes('TRUSTED_PROXY_CIDRS'), false);
});

// ══ Prod refusal, first-failure stop, deadline, canaries ════════════════
test('production identifiers are refused structurally', () => {
  for (const id of PROD_DENYLIST) {
    assert.throws(() => assertNoProdIds([id]), (e) => e.code === E.PROD_ID_REFUSED, id);
  }
  assert.throws(() => extractComposeId({ composeId: PROD_DENYLIST[0] }),
    (e) => e.code === E.PROD_ID_REFUSED);
  assert.throws(() => projectStagingBinding(projectBody({
    composes: [{ composeId: '_A6rI-GEm9oF8ysIojm0O', appName: 'x' }] })),
    (e) => e.code === E.PROD_ID_REFUSED);
});

test('the sequence stops at the first non-success; later calls never happen', async () => {
  for (const [failPath, later] of [
    [P_COMPOSE_CREATE, [P_COMPOSE_UPDATE, P_DOMAIN_CREATE, P_COMPOSE_DEPLOY]],
    [P_COMPOSE_UPDATE, [P_DOMAIN_CREATE, P_COMPOSE_DEPLOY]],
    [P_DOMAIN_CREATE, [P_COMPOSE_DEPLOY]],
  ]) {
    const h = harness({ failAt: failPath });
    await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.UPSTREAM_STATUS);
    for (const l of later) assert.ok(!h.calls.includes(`POST ${l}`), `${l} after ${failPath}`);
    assert.equal(h.cleanedCount(), 1);
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

test('deadline with an active forward: E_DEADLINE and cleanup exactly once', async () => {
  const h = harness();
  const inner = h.args.requestFn;
  h.args.requestFn = (o) => o.path.startsWith(P_PROJECT_ONE)
    ? new Promise((r) => setTimeout(() => r(projectBody()), 400))
    : inner(o);
  await assert.rejects(() => orchestrate({ ...h.args, deadlineMs: 20 }), (e) => e.code === E.DEADLINE);
  assert.equal(h.cleanedCount(), 1);
});

test('token rides only in x-api-key', () => {
  const o = buildRequestOptions({ socketPath: '/s', token: TOKEN_CANARY,
                                  path: P_COMPOSE_DEPLOY, method: 'POST', bodyJson: '{}' });
  assert.equal(o.headers['x-api-key'], TOKEN_CANARY);
  assert.ok(!o.path.includes(TOKEN_CANARY));
  for (const [k, v] of Object.entries(o.headers)) {
    if (k !== 'x-api-key') assert.ok(!String(v).includes(TOKEN_CANARY), k);
  }
  assert.ok(!('authorization' in o.headers) && !('cookie' in o.headers));
});

test('fixed bodies pin the intended target', () => {
  assert.equal(bodyComposeCreate(ENV_ID).environmentId, ENV_ID);
  assert.equal(bodyComposeSource(COMPOSE_ID).branch, INFRA_BRANCH);
  const d = bodyDomainCreate(COMPOSE_ID);
  assert.equal(d.host, DOMAIN_HOST);
  assert.equal(d.port, DOMAIN_PORT);
  assert.equal(d.serviceName, DOMAIN_SERVICE);
  assert.deepEqual(bodyComposeDeploy(COMPOSE_ID), { composeId: COMPOSE_ID });
});

test('projections allowlist structural fields and drop env', () => {
  const st = projectComposeState(COMPOSE_ROW);
  assert.ok(!('env' in st) && !JSON.stringify(st).includes(SECRET_CANARY));
  const [d] = projectDomainRows([{ ...DOMAIN_ROW, env: SECRET_CANARY }]);
  assert.ok(!('env' in d) && !JSON.stringify(d).includes(SECRET_CANARY));
  assert.equal(domainMatchesTarget(DOMAIN_ROW), true);
  assert.equal(composeMatchesTarget(st), true);
});

test('canaries are absent from every failure surface', async () => {
  for (const failAt of [P_COMPOSE_CREATE, P_COMPOSE_ONE, P_DOMAIN_CREATE, P_COMPOSE_DEPLOY]) {
    const h = harness({ failAt });
    const e = await orchestrate(h.args).then(() => null, (err) => err);
    const s = surface(e);
    assert.ok(!s.includes(TOKEN_CANARY), 'token leaked on ' + failAt);
    assert.ok(!s.includes(SECRET_CANARY), 'secret leaked on ' + failAt);
  }
});

test('the success receipt carries no secret and no token', async () => {
  const h = harness();
  const r = await orchestrate(h.args);
  const receipt = [OK_RECEIPT, r.composeId, r.environmentId, r.credPath,
                   r.domainAction, r.escrowReused].join('|');
  assert.ok(!receipt.includes(SECRET_CANARY) && !receipt.includes(TOKEN_CANARY));
});


// ══ Cipher C1: the CREATED row is held to the documented shape ══════════
test('C1 REPRO: a create returning a foreign source shape is REFUSED', async () => {
  // Exactly Cipher's probe: correct new id/environment/appName, but
  // sourceType=gitlab, composePath=./foreign.yml, repository/owner/branch null.
  const h = harness({ composeRow: { ...COMPOSE_ROW, sourceType: 'gitlab',
                                    composePath: './foreign.yml',
                                    repository: null, owner: null, branch: null } });
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.COMPOSE_CONFLICT);
  assert.ok(!h.calls.includes(`POST ${P_COMPOSE_UPDATE}`), 'no update on a foreign shape');
  assert.ok(!h.calls.includes(`POST ${P_DOMAIN_CREATE}`));
  assert.ok(!h.calls.includes(`POST ${P_COMPOSE_DEPLOY}`));
});

test('C1: the untouched-stub grammar is exact, not truthiness', () => {
  const stub = { sourceType: 'github', composePath: './docker-compose.staging.yml',
                 repository: null, owner: null, branch: null };
  assert.equal(isUntouchedStub(stub), true);
  assert.equal(isUntouchedStub({ ...stub, sourceType: 'gitlab' }), false, 'wrong sourceType');
  assert.equal(isUntouchedStub({ ...stub, composePath: './foreign.yml' }), false, 'wrong path');
  // Empty strings are NOT unset: `!x` accepted these before.
  assert.equal(isUntouchedStub({ ...stub, repository: '' }), false);
  assert.equal(isUntouchedStub({ ...stub, owner: '' }), false);
  assert.equal(isUntouchedStub({ ...stub, branch: '' }), false);
});

// ══ Cipher C2: escrow.reused is not ownership ═══════════════════════════
test('C2: an unbound candidate with NO intent marker is refused', () => {
  const b = projectStagingBinding(projectBody({ composes: EXISTING }));
  assert.throws(() => selectTarget(b, null, null), (e) => e.code === E.ADOPT_NO_MARKER);
});

test('C2: a candidate whose appName does not carry OUR nonce is refused', () => {
  const foreign = [{ composeId: 'cmp_x', appName: COMPOSE_APPNAME + '-ffffffffffffffff-zz' }];
  const b = projectStagingBinding(projectBody({ composes: foreign }));
  assert.throws(() => selectTarget(b, null, { nonce: NONCE }), (e) => e.code === E.ADOPT_NO_MARKER);
  assert.equal(appNameCarriesNonce(intendedAppName(NONCE), NONCE), true);
  assert.equal(appNameCarriesNonce(intendedAppName(NONCE) + '-ab12cd', NONCE), true);
  assert.equal(appNameCarriesNonce(COMPOSE_APPNAME + '-deadbeefdeadbeef', NONCE), false);
});

test('C2: the create request carries the nonce-bearing appName', async () => {
  const h = harness();
  const bodies = [];
  const inner = h.args.requestFn;
  h.args.requestFn = (o) => { bodies.push([o.path.split('?')[0], o.body]); return inner(o); };
  await orchestrate(h.args);
  const create = bodies.find(([p]) => p === P_COMPOSE_CREATE);
  assert.equal(create[1].appName, intendedAppName(NONCE));
});

// ══ Cipher C3: bounded printable identifier grammar ═════════════════════
test('C3 REPRO: control-bearing and oversize identifiers are rejected', () => {
  for (const bad of ['env-ok\nXEROX_QUIZ_STAGING_PROVISIONED',
                     '\u001b[2Jenv', 'a'.repeat(200), '', 'sp ace', 'quote"x']) {
    assert.throws(() => assertId(bad, E.BAD_SHAPE), (e) => e.code === E.BAD_SHAPE,
      JSON.stringify(bad).slice(0, 40));
  }
  assert.equal(assertId('env_staging_QUIZ-1.a:b@c+d', E.BAD_SHAPE), 'env_staging_QUIZ-1.a:b@c+d');
});

test('C3: a forged environmentId cannot reach the receipt', () => {
  assert.throws(() => projectStagingBinding(projectBody({
    envOverride: { environmentId: 'env\nXEROX_QUIZ_STAGING_PROVISIONED' } })),
    (e) => e.code === E.BAD_SHAPE);
  assert.throws(() => projectStagingBinding(projectBody({
    composes: [{ composeId: 'ok', appName: 'bad\u001b[2J' }] })), (e) => e.code === E.BAD_SHAPE);
});

// ══ Cipher C4: the reviewed revision is enforced ════════════════════════
test('C4: the revision gate accepts only the exact reviewed SHA on the exact branch', () => {
  assert.equal(assertInfraRevision(GOOD_LSREMOTE), true);
  for (const bad of [
    '0000000000000000000000000000000000000000\trefs/heads/aperture-ztid5-staging\n', // moved
    '26d6a9eb25b8653356d1aae4659382a62d479c41\trefs/heads/main\n',                    // wrong branch
    '',                                                                                // deleted
    'not-a-sha\trefs/heads/aperture-ztid5-staging\n',
    GOOD_LSREMOTE + GOOD_LSREMOTE,                                                     // ambiguous
  ]) {
    assert.throws(() => assertInfraRevision(bad), (e) => e.code === E.INFRA_REV_MISMATCH);
  }
});

test('C4: a moved branch aborts BEFORE the forward and before any call', async () => {
  let forwards = 0;
  const h = harness({ lsRemote: '0000000000000000000000000000000000000000\trefs/heads/aperture-ztid5-staging\n' });
  h.args.openForwardFn = () => { forwards += 1; return { ready: Promise.resolve('/s'), cleanup: () => {} }; };
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.INFRA_REV_MISMATCH);
  assert.equal(forwards, 0, 'must abort before the forward opens');
  assert.deepEqual(h.posts(), []);
  assert.ok(!h.calls.includes('readToken'), 'and before the token is read');
});


test('C1: a fresh create returning the UNTOUCHED STUB is accepted and proceeds', () => {
  // The realistic create response: Dokploy returns an unconfigured stub, not a
  // fully-configured row. Without this case the shape gate could be removed
  // entirely and every other test would still pass, because a configured row
  // also satisfies the exact-target branch. This is the case that makes the
  // gate observable.
  const stub = { composeId: COMPOSE_ID, appName: intendedAppName(NONCE),
                 environmentId: ENV_ID, sourceType: 'github',
                 composePath: './docker-compose.staging.yml',
                 repository: null, owner: null, branch: null };
  assert.equal(isUntouchedStub(projectComposeState(stub)), true);
  assert.equal(assertAdoptable(projectComposeState(stub)), true);
  // and it must NOT satisfy the configured-target test
  assert.equal(composeMatchesTarget(projectComposeState(stub)), false);
});

test('C1: fresh create end-to-end with a stub response completes all mutations', async () => {
  const stub = { composeId: COMPOSE_ID, appName: intendedAppName(NONCE),
                 environmentId: ENV_ID, sourceType: 'github',
                 composePath: './docker-compose.staging.yml',
                 repository: null, owner: null, branch: null };
  const h = harness({ intent: null, composeRow: stub });
  const r = await orchestrate(h.args);
  assert.equal(r.composeId, COMPOSE_ID);
  for (const p of [P_COMPOSE_UPDATE, P_DOMAIN_CREATE, P_COMPOSE_DEPLOY]) {
    assert.ok(h.calls.includes(`POST ${p}`), 'expected ' + p);
  }
});


test('C2 REPRO: created row whose re-read appName carries a FOREIGN nonce is refused', async () => {
  // Request nonce A, compose.one returns an exact untouched stub with nonce B.
  const foreign = { composeId: COMPOSE_ID, appName: COMPOSE_APPNAME + '-ffffffffffffffff',
                    environmentId: ENV_ID, sourceType: 'github',
                    composePath: './docker-compose.staging.yml',
                    repository: null, owner: null, branch: null };
  const h = harness({ intent: null, composeRow: foreign });
  await assert.rejects(() => orchestrate(h.args), (e) => e.code === E.ADOPT_NO_MARKER);
  for (const p of [P_COMPOSE_UPDATE, P_DOMAIN_CREATE, P_COMPOSE_DEPLOY]) {
    assert.ok(!h.calls.includes(`POST ${p}`), p + ' must not run');
  }
});
