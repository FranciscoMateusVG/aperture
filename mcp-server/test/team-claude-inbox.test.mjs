import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, writeFileSync, chmodSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runInNewContext } from 'node:vm';
import { claudeInboxProbeSchema, parseClaudeInboxProbe } from '../dist/team-claude-inbox.js';
import { invokeTeamControl, teamControlWatchdogMs } from '../dist/team-control.js';

const fixture = name => JSON.parse(readFileSync(new URL(`../../tests/fixtures/team-claude-inbox-${name}.json`, import.meta.url), 'utf8'));
const request = fixture('request');
const input = request.input;

test('shared native wire proves kickoff sent but not tools or public readiness', () => {
  assert.deepEqual(claudeInboxProbeSchema.parse(input), input);
  const result = parseClaudeInboxProbe(fixture('response'), input).result;
  assert.equal(result.startup, 'verified');
  assert.equal(result.kickoff, 'sent');
  assert.equal(result.owner_state, 'quarantined');
  assert.equal(result.cleanup, 'verified');
  assert.equal(result.mcp_readiness, 'pending_verification');
  assert.equal(result.public_enabled, false);
});

test('strict smoke selectors never accept caller authority, prompts or a second generation', () => {
  for (const delta of [{ actor: 'glados' }, { model: 'sonnet' }, { prompt: 'go' },
    { force: true }, { timeout: 100 }, { token: 'fixture' }, { expected_generation: 1 },
    { expected_generation: -1 }, { expected_generation: '0' }, { expected_generation: 0.5 },
    { team: '../x' }, { seat: 'x/y' }, { team: 'x'.repeat(17) }, { seat: 'x'.repeat(32) }]) {
    assert.equal(claudeInboxProbeSchema.safeParse({ ...input, ...delta }).success, false);
  }
});

test('mismatched or overstated diagnostic receipts are UNKNOWN without retry authority', () => {
  for (const delta of [{ team: 'other' }, { seat: 'other' }, { generation: 2 },
    { model: 'sonnet' }, { reasoning_observation: 'high' }, { startup: 'assumed' },
    { kickoff: 'ready' }, { owner_state: 'active' }, { cleanup: 'quarantined' }, { mcp_readiness: 'passed' }, { public_enabled: true },
    { thread_id: 'caller' }, { token: 'fixture' }]) {
    const value = fixture('response'); Object.assign(value.result, delta);
    assert.throws(() => parseClaudeInboxProbe(value, input), /E_CONTROL_UNKNOWN/);
  }
  for (const value of [null, {}, [], { ...fixture('response'), action: 'bootstrap_seat' }]) {
    assert.throws(() => parseClaudeInboxProbe(value, input), /E_CONTROL_UNKNOWN/);
  }
});

function handler(denied, invoke) {
  const source = readFileSync(new URL('../dist/index.js', import.meta.url), 'utf8');
  const start = source.indexOf('server.tool("team_claude_inbox_probe",');
  const end = source.indexOf('\nserver.tool(', start + 1);
  assert.ok(start >= 0 && end > start);
  let callback;
  runInNewContext(source.slice(start, end), {
    server: { tool(_name, description, schema, fn) {
      assert.match(description, /Historical diagnostic, retired for new launches/);
      assert.match(description, /Use team_bootstrap_seat/);
      assert.equal(schema.input, claudeInboxProbeSchema); callback = fn;
    } },
    gladosControlDenied: () => denied, claudeInboxProbeSchema, parseClaudeInboxProbe,
    invokeTeamControl: invoke,
  });
  return callback;
}

test('emitted handler authenticates before parsing and invokes one diagnostic, never retries', async () => {
  let calls = 0;
  const deny = { isError: true };
  assert.equal(await handler(deny, async () => { calls++; })({ input: { actor: 'glados' } }), deny);
  assert.equal(calls, 0);
  const run = handler(null, async value => {
    calls++; assert.equal(JSON.stringify(value), JSON.stringify(request)); return fixture('response');
  });
  assert.equal((await run({ input: { ...input, prompt: 'go' } })).isError, true);
  assert.equal(calls, 0);
  assert.deepEqual(JSON.parse((await run({ input })).content[0].text), fixture('response'));
  assert.equal(calls, 1);
  const fail = handler(null, async () => { calls++; throw new Error('E_CONTROL_UNKNOWN'); });
  assert.equal((await fail({ input })).isError, true);
  assert.equal(calls, 2);
});

test('native diagnostic transport uses stdin, isolated process group and fixed 180s watchdog', async () => {
  assert.equal(teamControlWatchdogMs('claude_inbox_probe'), 180_000);
  const root = mkdtempSync(join(tmpdir(), 'aperture-smoke-wire-'));
  const bin = join(root, 'control');
  const got = join(root, 'request');
  writeFileSync(bin, `#!/bin/sh\n[ "$#" -eq 0 ] || exit 70\nPGID=$(ps -p $$ -o pgid= | tr -d ' ')\n[ "$PGID" = "$$" ] || exit 71\nIFS= read -r value\nprintf '%s' "$value" > '${got}'\nprintf '%s\\n' '${JSON.stringify(fixture('response'))}'\n`);
  chmodSync(bin, 0o700);
  const previous = process.env.APERTURE_TEAM_CONTROL_BIN;
  process.env.APERTURE_TEAM_CONTROL_BIN = bin;
  try {
    assert.deepEqual(await invokeTeamControl(request), fixture('response'));
    assert.deepEqual(JSON.parse(readFileSync(got, 'utf8')), request);
  } finally {
    if (previous === undefined) delete process.env.APERTURE_TEAM_CONTROL_BIN;
    else process.env.APERTURE_TEAM_CONTROL_BIN = previous;
    rmSync(root, { recursive: true, force: true });
  }
});
