import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createTeamSchema, parseCreatedTeam } from '../dist/team-create.js';
const request = JSON.parse(readFileSync(new URL('./fixtures/team-create-request.json', import.meta.url), 'utf8'));
const uuid = '11111111-1111-4111-8111-111111111111';
const clone = v => structuredClone(v);
function result(input = request.input) {
  const counts = new Map();
  const seats = input.seats.map(s => {
    const count = (counts.get(s.role) ?? 0) + 1; counts.set(s.role,count);
    return {...s, name: `${input.team}-${s.role}${count === 1 ? '' : `-${count}`}`};
  });
  return { action: 'create', result: {
    team: {
      snapshot: { schema_version:1, ...clone(input), preset:{id:input.preset_id, sha256:input.preset_id ? 'a'.repeat(64) : null},
        lead:seats[input.lead_index].name, seats, grants:[], created_at:'fixture', creation_request_id:uuid, staging_uuid:uuid },
      state: {state:'pending',generation:0,epic_id:null},
      seats:seats.map(configured => ({configured,observed_owner:null})),
      capabilities:{cancel:true,activate:true,start:false,checkpoint:false,replace:false,archive:false},
    },
    creation_request:{schema_version:1,request_id:uuid,team:input.team,project:input.project,repo:input.repo,
      snapshot_sha256:'a'.repeat(64),expected_generation:0,created_at:'fixture'},
  }};
}
test('shared real Rust command fixture parses without envelope renaming', () => {
  const input = createTeamSchema.parse(request.input);
  assert.deepEqual(input,request.input);
  assert.equal(parseCreatedTeam(result(),input).result.team.state.state,'pending');
});
test('write input rejects caller authority and invalid team composition', () => {
  for (const key of ['actor','grants','source','created_at','creation_request_id']) {
    assert.throws(() => createTeamSchema.parse({...request.input,[key]:'glados'}));
  }
  assert.throws(() => createTeamSchema.parse({...request.input,seats:request.input.seats.slice(0,1)}));
  assert.throws(() => createTeamSchema.parse({...request.input,lead_index:2}));
  assert.throws(() => createTeamSchema.parse({...request.input,seats:[{...request.input.seats[0],name:'forged'},request.input.seats[1]]}));
});
test('response requires pending state and exact immutable request correspondence', () => {
  const mutations = [
    r => r.result.team.state.state='active', r => r.result.team.state.generation=1,
    r => r.result.team.snapshot.mission='other', r => r.result.team.snapshot.repo='other',
    r => r.result.team.snapshot.seats[0].model='other', r => r.result.team.snapshot.lead='other',
    r => r.result.team.capabilities.start=true, r => r.result.team.seats[0].observed_owner={},
    r => r.result.creation_request.request_id='22222222-2222-4222-8222-222222222222',
    r => r.result.team.snapshot.grants=[{scope:'message'}],
  ];
  for (const mutate of mutations) { const r=result();mutate(r);assert.throws(() => parseCreatedTeam(r,request.input),/E_CONTROL_FAILED/); }
});
test('duplicate roles use backend canonical suffixes and nonzero lead index', () => {
  const input=clone(request.input);input.seats[1]=clone(input.seats[0]);input.lead_index=1;
  assert.equal(parseCreatedTeam(result(input),input).result.team.snapshot.lead,'conversation-backend-2');
});
