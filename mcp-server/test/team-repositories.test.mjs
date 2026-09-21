import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseRepositoryRegistry, parseSavedRepository, saveRepositorySchema } from '../dist/team-repositories.js';

// Shared with the Rust side: the native test sends REQUEST through team_control_headless and compares real serde output to RESPONSE.
const fixture = name => JSON.parse(readFileSync(new URL(`./fixtures/${name}.json`, import.meta.url), 'utf8'));
const request = fixture('repository-save-request');
const response = fixture('repository-save-response');

const sha = 'a'.repeat(64), sha2 = 'b'.repeat(64);
const clone = v => structuredClone(v);
const input = { project: 'project:incluir', repo: 'eunenem-engine', display_name: 'EuNeném Engine', enabled: true, expected_sha256: sha };
const seeds = [
  { project: 'project:aperture', repo: 'aperture', display_name: 'Aperture', enabled: true, available: true },
  { project: 'project:incluir', repo: 'eunenem', display_name: 'EuNeném', enabled: true, available: false },
];
// Every response is built from fresh clones: mutation cases must never leak into later tests.
const view = (repositories = clone(seeds), digest = sha) => ({ schema_version: 1, sha256: digest, repositories });
const list = (v = view()) => ({ action: 'list_repositories', result: v });
const saved = (i = input, digest = sha2) => ({ action: 'save_repository', result: view([...clone(seeds), { project: i.project, repo: i.repo, display_name: i.display_name, enabled: i.enabled, available: true }], digest) });
// Control (Cc) and bidi controls: the same rejection set as the native validate_text.
const UNSAFE_DISPLAY = ["", "   ", "a\tb", "a\u0001b", "a\u007fb", "a\u0085b", "a\u061cb", "a\u200eb", "a\u200fb", "a\u202ab", "a\u202eb", "a\u2066b", "a\u2069b", "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", "\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9"];
const X_BIDI_Y = "x\u202ey";

test('shared native fixtures: request parses unchanged and the real serde response is an exact echo', () => {
  assert.equal(request.action, 'save_repository');
  assert.deepEqual(saveRepositorySchema.parse(request.input), request.input);
  const result = parseSavedRepository(response, request.input);
  assert.equal(result.sha256, response.result.sha256);
  assert.notEqual(result.sha256, request.input.expected_sha256, 'digest advances after a real save');
  const byKey = Object.fromEntries(result.repositories.map(r => [`${r.project} ${r.repo}`, r]));
  assert.equal(byKey['project:aperture aperture'].available, true);
  assert.deepEqual(byKey['project:incluir eunenem-engine'], { project: 'project:incluir', repo: 'eunenem-engine', display_name: request.input.display_name, enabled: true, available: true });
  assert.ok(result.repositories.length >= 5, 'seeds were materialized alongside the new entry');
  assert.throws(() => parseRepositoryRegistry(response, 'list_repositories'), /E_CONTROL_FAILED/, 'save fixture is not a list response');
});
test('save input mirrors native SaveRepositoryInput exactly and refuses authority or path fields', () => {
  assert.deepEqual(saveRepositorySchema.parse(input), input);
  for (const [key, value] of [['actor', 'glados'], ['grants', []], ['path', '/tmp/x'], ['root', '~/projects/x'], ['team', 't1'], ['activate', true], ['source', 'local']]) {
    assert.throws(() => saveRepositorySchema.parse({ ...input, [key]: value }), key);
  }
  for (const key of ['project', 'repo', 'display_name', 'enabled', 'expected_sha256']) {
    const missing = { ...input }; delete missing[key]; assert.throws(() => saveRepositorySchema.parse(missing), `missing ${key}`);
  }
  assert.throws(() => saveRepositorySchema.parse({ ...input, enabled: 'true' }), 'string enabled');
});
test('project taxonomy, repository key, display text and digest are validated like the native side', () => {
  for (const project of ['project:unknown', 'incluir', '']) assert.throws(() => saveRepositorySchema.parse({ ...input, project }), project);
  for (const repo of ['Eunenem', '-engine', 'a/b', '../x', 'x'.repeat(65), '']) assert.throws(() => saveRepositorySchema.parse({ ...input, repo }), repo);
  for (const repo of ['a', 'eunenem-engine', 'x.y_z-1']) assert.equal(saveRepositorySchema.parse({ ...input, repo }).repo, repo);
  for (const display_name of UNSAFE_DISPLAY) assert.throws(() => saveRepositorySchema.parse({ ...input, display_name }), JSON.stringify(display_name).slice(0, 24));
  assert.equal([...saveRepositorySchema.parse({ ...input, display_name: 'é'.repeat(80) }).display_name].length, 80);
  assert.equal(saveRepositorySchema.parse({ ...input, display_name: 'Programa Incluir — staging' }).display_name, 'Programa Incluir — staging');
  for (const expected_sha256 of ['A'.repeat(64), 'a'.repeat(63), 'a'.repeat(65), 'g'.repeat(64), '']) assert.throws(() => saveRepositorySchema.parse({ ...input, expected_sha256 }), expected_sha256.slice(0, 5) || 'empty');
});
test('list response validates exact native envelope shape and refuses duplicates or extra fields', () => {
  assert.deepEqual(parseRepositoryRegistry(list(), 'list_repositories'), view());
  assert.throws(() => parseRepositoryRegistry(list(view([])), 'list_repositories'), /E_CONTROL_FAILED/, 'native registry is never empty; an empty array is malformed');
  const mutations = [
    ['wrong action', r => { r.action = 'catalog'; }],
    ['save action for list', r => { r.action = 'save_repository'; }],
    ['extra envelope field', r => { r.actor = 'glados'; }],
    ['schema version', r => { r.result.schema_version = 2; }],
    ['uppercase digest', r => { r.result.sha256 = 'A'.repeat(64); }],
    ['duplicate binding', r => { r.result.repositories.push(clone(r.result.repositories[0])); }],
    ['path in entry', r => { r.result.repositories[0].path = '/tmp/x'; }],
    ['string available', r => { r.result.repositories[0].available = 'true'; }],
    ['missing enabled', r => { delete r.result.repositories[0].enabled; }],
    ['unsafe display', r => { r.result.repositories[0].display_name = X_BIDI_Y; }],
    ['project outside taxonomy', r => { r.result.repositories[0].project = 'project:other'; }],
    ['empty registry', r => { r.result.repositories = []; }],
    ['too many entries', r => { r.result.repositories = Array.from({ length: 129 }, (_, i) => ({ ...seeds[0], repo: `r${i}` })); }],
  ];
  for (const [name, mutate] of mutations) { const r = list(); mutate(r); assert.throws(() => parseRepositoryRegistry(r, 'list_repositories'), /E_CONTROL_FAILED/, name); }
});
test('save is confirmed only by an exact echo of the requested entry; nothing about teams is inferred', () => {
  const result = parseSavedRepository(saved(), input);
  assert.equal(result.sha256, sha2);
  assert.deepEqual(result.repositories.at(-1), { project: 'project:incluir', repo: 'eunenem-engine', display_name: 'EuNeném Engine', enabled: true, available: true });
  assert.equal('team' in result, false); assert.equal('teams' in result, false);
  const disabled = { ...input, enabled: false };
  assert.equal(parseSavedRepository(saved(disabled), disabled).repositories.at(-1).enabled, false);
  assert.equal(parseSavedRepository(saved(input, sha), input).sha256, sha, 'same digest is a legitimate no-op echo');
  const mutations = [
    ['list action for save', r => { r.action = 'list_repositories'; }],
    ['entry missing', r => { r.result.repositories.pop(); }],
    ['display name differs', r => { r.result.repositories.at(-1).display_name = 'Other'; }],
    ['enabled flipped', r => { r.result.repositories.at(-1).enabled = false; }],
    ['repo remapped', r => { r.result.repositories.at(-1).repo = 'eunenem'; r.result.repositories.splice(1, 1); }],
    ['project remapped', r => { r.result.repositories.at(-1).project = 'project:aperture'; }],
  ];
  for (const [name, mutate] of mutations) { const r = saved(); mutate(r); assert.throws(() => parseSavedRepository(r, input), /E_CONTROL_FAILED/, name); }
});
