// aperture-syzem: source/fixture contract tests, NOT proof of Rust rendering,
// live harness behavior, paid-session budgets or installation. P1/P3 integration
// must exercise the production renderer and fixed inbox with fresh sessions.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../../', import.meta.url));
const read = (path) => readFileSync(`${repo}${path}`, 'utf8');
const roles = ['backend', 'frontend', 'mobile', 'qa', 'security'];
const vars = ['seat_name', 'team_name', 'project', 'lead_name', 'role'];
const list = (path) => read(path).split('\n').map(line => line.split('#')[0].trim()).filter(Boolean);
const constitution = read('.claude/skills/constitution/SKILL.md');
const forbiddenControls = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\u061C\u200E\u200F\u202A-\u202E\u2066-\u2069]/u;
const preset = JSON.parse(read('teams/presets/fullstack.json'));

// A transparent five-literal substitution oracle to inspect our templates.
// No conditionals, includes, env reads, generic template engine or runtime claims.
function fixturePrompt(role, lead) {
  const values = { seat_name: `t1-${role}`, team_name: 't1', project: 'project:aperture', lead_name: lead, role };
  return read(`roles/${role}/prompt.md.tmpl`).replace(/\{\{([a-z_]+)\}\}/g, (_, name) => {
    assert.ok(vars.includes(name), `unknown template variable ${name}`);
    return values[name];
  });
}

test('five specialist roles exist; every lazy skill resolves and exactly two cores are resident', () => {
  assert.deepEqual(readdirSync(`${repo}roles`).filter(name => existsSync(`${repo}roles/${name}/prompt.md.tmpl`)).sort(), roles);
  for (const role of roles) {
    const residents = list(`roles/${role}/resident.txt`);
    assert.deepEqual(residents, ['constitution', `${role}-core`]);
    const skills = list(`roles/${role}/skills.txt`);
    assert.equal(skills.length, new Set(skills).size);
    for (const skill of residents) assert.ok(skills.includes(skill));
    for (const skill of skills) {
      const path = skill === `${role}-core` ? `roles/${role}/core/SKILL.md` : `.claude/skills/${skill}/SKILL.md`;
      assert.ok(existsSync(`${repo}${path}`), `${role}: skill ${skill} resolves`);
    }
    const core = read(`roles/${role}/core/SKILL.md`);
    assert.match(core, new RegExp(`^---\\nname: ${role}-core\\ndescription:`, 'u'));
    assert.doesNotMatch(core, /# Skill:/u, 'core must not smuggle additional injected skills');
    const injected = ['constitution', `${role}-core`].map((name, i) => `\n# Skill: ${name}\n${i ? core : constitution}`).join('');
    assert.equal((injected.match(/^# Skill:/gm) || []).length, 2);
  }
});

test('templates use ONLY fixed contract variables; keep harness boot outside the template', () => {
  for (const role of roles) {
    const template = read(`roles/${role}/prompt.md.tmpl`);
    const used = [...template.matchAll(/\{\{([^}]+)\}\}/g)].map(m => m[1]);
    assert.deepEqual([...new Set(used)].sort(), [...vars].sort());
    assert.ok(!forbiddenControls.test(template));
    assert.doesNotMatch(template, /\$\{|\{\{%|\{%|\{\{>|\{\{\{/u);
    assert.doesNotMatch(template, /hub-client|Monitor tool|start your inbox monitor|APERTURE_HUB_TOKEN/u,
      'only the harness-specific runtime suffix can request a monitor');
    for (const lead of [`t1-${role}`, 't1-other']) {
      const prompt = fixturePrompt(role, lead);
      assert.doesNotMatch(prompt, /\{\{|\}\}/u);
      assert.ok(prompt.includes(`**t1-${role}**`) && prompt.includes('**t1**') && prompt.includes('**project:aperture**'));
      assert.ok(prompt.includes(`**${lead}**`));
      assert.ok(prompt.includes(`your seat t1-${role} equals the snapshot lead ${lead}`));
      assert.match(prompt, /\*\*Worker \(seat is not lead\):\*\*/u);
      assert.match(prompt, /\*\*Lead \(seat equals lead\):\*\*/u);
    }
  }
});

test('both harness contracts preserve checkpoint distinction and fail honestly if unavailable', () => {
  for (const role of roles) {
    const prompt = fixturePrompt(role, 't1-backend');
    assert.match(prompt, /Claude uses explicit milestone checkpoint tool writes plus a best-effort Stop hook/u);
    assert.match(prompt, /Codex uses the explicit tool\/protocol only: there is no Codex Stop hook/u);
    assert.match(prompt, /If the tool is unavailable, report the capability gap/u);
    assert.match(prompt, /never secrets/u);
    assert.match(prompt, /Do not send transcript\/env\/tool arguments, provider URLs or unbounded text/u);
    assert.match(prompt, /writer assigns schema_version, checkpoint_id=seat\/g\/seq/u);
    assert.match(prompt, /Never invent validation or manually edit checkpoint files/u);
    assert.match(prompt, /reconcile git\/PR\/deploy reality before repeating an effect/u);
  }
});

test('lead guidance carries creation gate, batching, scoped routing and archive reconciliation', () => {
  for (const role of roles) {
    const prompt = fixturePrompt(role, `t1-${role}`);
    for (const text of [
      'only GLaDOS files beads, only after operator acknowledgement',
      'actual seat assignees', 'never the global queue', 'one report',
      'immediate delta in the next turn', 'same project', 'explicit trusted-registry grant',
      'written acceptance and re-parenting', 'Preserve history', 'open epic child',
      'unmet success metric blocks archive', 'No ready/list/search sweeps',
      'Models/fallbacks in presets are editable suggestions', 'Handoff wakes QA', 'PR-open ends nothing', 'Publication is explicit',
    ]) assert.ok(prompt.includes(text), `${role}: missing ${text}`);
    assert.doesNotMatch(prompt, /\b(?:create_task|bd create)\b/u, 'no actionable bead-creation recipe in worker prompt');
  }
});

test('handoff wakes QA, verdict wakes lead/root, PR-open ends nothing, publication explicit, no tools is a blocker', () => {
  for (const role of roles) {
    const prompt = fixturePrompt(role, `t1-${role}`);
    const worker = prompt.slice(prompt.indexOf('**Worker (seat is not lead):**'), prompt.indexOf('**Lead (seat equals lead):**'));
    const lead = prompt.slice(prompt.indexOf('**Lead (seat equals lead):**'), prompt.indexOf('For both: sender/assignee'));
    assert.ok(worker.length > 0 && lead.length > 0, `${role}: both sections present`);
    for (const text of ['immutable head SHA', 'never notes-only', "reviewer's receipt", 'BLOCKER: messaging tools unavailable', 'no handoff counts as delivered']) {
      assert.ok(worker.includes(text), `${role}: worker section missing ${text}`);
    }
    for (const text of [
      'Handoff wakes QA', 'immutable head SHA, files, PR URL and base', 'A note on a bead is evidence, not a handoff',
      "stays OPEN until the reviewer's receipt message names that SHA", 'never assume delivery',
      'Verdict wakes lead and root', 'PASS or HOLD', 'to the lead and to GLaDOS', 'A verdict left only in notes is not a verdict',
      'PR-open ends nothing', "closes neither the task's responsibility nor the mission", 'never on PR-open',
      'Publication is explicit', 'release target (branch/environment), actor and authority', 'Never infer permission to merge or promote to main/prod', 'a blocker for GLaDOS, not a default',
      'No tools means BLOCKER', 'never a completed handoff',
    ]) assert.ok(lead.includes(text), `${role}: lead section missing ${text}`);
    // Handoff and verdict are messages; notes are never presented as the delivery mechanism.
    assert.doesNotMatch(lead, /notes? (?:is|are|counts? as) (?:a |the )?(?:handoff|delivery|verdict)/iu);
    // No inferred publication authority anywhere in the rendered prompt.
    assert.doesNotMatch(prompt, /(?:may|can) (?:merge|promote) (?:to )?(?:main|prod)/iu);
  }
  // The specific runtime doc states the same completion contract for GLaDOS and the operator.
  const doc = read('docs/runtime/conversation-teams.md');
  for (const text of ['Mission completion', 'PR-open ends nothing', 'receipt', 'PASS or HOLD', 'frozen', 'BLOCKER: messaging tools unavailable']) {
    assert.ok(doc.includes(text), `conversation-teams.md missing ${text}`);
  }
});

test('fullstack matches frozen preset shape and suggests exact editable tuples, not approval', () => {
  assert.deepEqual(Object.keys(preset).sort(), ['schema_version','id','display_name','mission_placeholder','acceptance_placeholder','seats','lead_index','fallbacks','source'].sort());
  assert.equal(preset.schema_version, 1);
  assert.equal(preset.id, 'fullstack');
  assert.equal(preset.source, 'shipped');
  assert.equal(preset.lead_index, 0);
  assert.deepEqual(preset.seats.map(s => s.role), ['backend', 'frontend', 'qa']);
  assert.match(preset.acceptance_placeholder, /do not authorize paid sessions/u);
  for (const seat of preset.seats) {
    assert.deepEqual(Object.keys(seat).sort(), ['role','harness','model','reasoning'].sort());
    assert.ok(roles.includes(seat.role));
  }
  for (const tuple of [...preset.seats, ...preset.fallbacks]) {
    assert.ok(['claude', 'codex'].includes(tuple.harness));
    assert.ok(typeof tuple.model === 'string' && tuple.model.length > 0);
    assert.equal(tuple.reasoning, tuple.harness === 'claude' ? null : 'high');
    assert.doesNotMatch(tuple.model, /^codex\//u, 'harness is a separate field');
  }
  for (const tuple of preset.fallbacks) assert.deepEqual(Object.keys(tuple).sort(), ['harness','model','reasoning'].sort());
  assert.deepEqual(preset.fallbacks.map(f => f.model), ['gpt-5.6-sol', 'gpt-5.6-terra']);
});

test('constitution decision sentences remain resident and V4 supersessions are explicit', () => {
  const decisions = read('.claude/skills/constitution/DECISIONS.md');
  const rows = decisions.split('\n').filter(line => /^\| C-\d+ \|/u.test(line));
  assert.equal(rows.length, 18);
  for (const row of rows) {
    const rule = row.split(' | ')[1];
    assert.ok(constitution.includes(rule), `missing resident sentence ${row.split(' | ')[0]}`);
  }
  for (let i = 1; i <= 7; i++) assert.ok(decisions.includes(`V4-D${i}`));
  const team = read('.claude/skills/team/SKILL.md');
  assert.match(team, /not a boot-time roster sweep/u);
  assert.match(team, /M3 timing belongs to the operator/u);
  assert.match(team, /same-project/u);
  for (const name of ['glados', 'wheatley', 'peppy']) {
    const prompt = read(`prompts/${name}.md`);
    assert.match(prompt, /actual harness/u);
    assert.match(prompt, /Codex app-server session/u);
    assert.ok(prompt.includes(`hub-client.js ${name}`), 'Claude branch keeps its own principal');
    assert.doesNotMatch(prompt, /On session start: start your inbox monitor/u);
  }
  assert.match(read('prompts/glados.md'), /lead of team leads/u);
  for (const name of ['wheatley', 'peppy']) assert.match(read(`prompts/${name}.md`), /You do not own their missions or direct their workers/u);
});

test('source fixtures contain no secret sentinels, control injection, or model authorization flag', () => {
  const sources = [JSON.stringify(preset), ...roles.flatMap(role => [read(`roles/${role}/prompt.md.tmpl`), read(`roles/${role}/core/SKILL.md`)])];
  for (const source of sources) {
    assert.ok(!forbiddenControls.test(source));
    assert.doesNotMatch(source, /SENTINEL_V4_SECRET_[AB]|(?:OPENAI_API_KEY|ANTHROPIC_API_KEY)\s*=/u);
  }
  assert.ok(!('approved' in preset) && !('authorized' in preset));
});

// P1 v2 bounded input contract (aperture-4yk4o, SHA1ca2f75b). These are
// shipped fixture bounds, not validation of arbitrary runtime user inputs.
test('shipped text/catalog/render source fit frozen P1 v2 budgets', () => {
  for (const value of [preset.display_name, preset.mission_placeholder, preset.acceptance_placeholder]) {
    assert.ok(value.trim().length > 0 && [...value].length <= 80);
    assert.ok(Buffer.byteLength(value) <= 320);
    assert.ok(!forbiddenControls.test(value));
  }
  assert.ok(preset.seats.length >= 1 && preset.seats.length <= 99);
  assert.ok(preset.fallbacks.length <= 16);
  assert.ok(Buffer.byteLength(read('teams/presets/fullstack.json')) <= 256 * 1024);
  const treeBytes = (dir) => readdirSync(dir, { withFileTypes: true }).reduce((total, item) => {
    const path = `${dir}/${item.name}`;
    return total + (item.isDirectory() ? treeBytes(path) : statSync(path).size);
  }, 0);
  let totalTeam = 0;
  for (const role of roles) {
    assert.ok(Buffer.byteLength(read(`roles/${role}/prompt.md.tmpl`)) <= 128 * 1024);
    const skills = list(`roles/${role}/skills.txt`);
    assert.ok(skills.length <= 64);
    // Conservative source estimate includes all skill reference files and a
    // 4 KiB allowance for the P1 fixed inbox/manifest/markers. Actual staged
    // output size is still checked by the production renderer at integration.
    const skillBytes = skills.reduce((total, skill) => total + treeBytes(`${repo}${skill === `${role}-core` ? `roles/${role}/core` : `.claude/skills/${skill}`}`), 0);
    const size = skillBytes + Buffer.byteLength(fixturePrompt(role, 't1-backend')) + 4096;
    assert.ok(size <= 256 * 1024, `${role}: conservative seat estimate ${size} exceeds cap`);
    if (preset.seats.some(seat => seat.role === role)) totalTeam += size;
  }
  assert.ok(totalTeam <= 8 * 1024 * 1024);
});
