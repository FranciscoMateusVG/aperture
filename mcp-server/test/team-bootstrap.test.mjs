import test from 'node:test';
import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
import {CLAUDE_EXACT_MODELS, bootstrapSeatSchema, bootstrapSelection, parseBootstrapStarted, parseTeamList} from '../dist/team-bootstrap.js';
import {teamControlWatchdogMs} from '../dist/team-control.js';
import {projectLabelSchema, saveRepositorySchema} from '../dist/team-repositories.js';
import {createTeamSchema} from '../dist/team-create.js';
const tuple = {harness:'codex',model:'gpt-5.6-sol',reasoning:'high'};
const seat = {...tuple,name:'mural-frontend',role:'frontend'};
const input = {team:'mural',seat:seat.name,expected_generation:0};
const owner = () => ({generation:0,state:'stale',configured:tuple,actual:null,process_count:0,thread_bound:false});
const list = () => ({action:'list_teams',result:[{snapshot:{team:'mural',project:'project:eunenem-engine',repo:'eunenem-engine',lead:seat.name,seats:[seat]},state:{state:'active',generation:1},seats:[{configured:seat,observed_owner:owner()}],capabilities:{start:true}}]});
const started = () => ({action:'bootstrap_seat',result:{team:'mural',seat:seat.name,generation:1,phase:'started',owner:{...owner(),generation:1,state:'active',actual:tuple,process_count:1,thread_bound:true},blockers:[]}});
test('GLaDOS bootstrap selectors carry no caller authority and each start has lifecycle watchdog',()=>{
 assert.deepEqual(bootstrapSeatSchema.parse(input),input);
 for(const extra of [{actor:'glados'},{model:'other'},{timeout:999},{expected_generation:1},{seat:'../x'}]) assert.equal(bootstrapSeatSchema.safeParse({...input,...extra}).success,false);
 assert.equal(teamControlWatchdogMs('bootstrap_seat'),180000);
 assert.equal(teamControlWatchdogMs('list_teams'),15000);
});
test('approved team state selects only exact never-started Codex seat, never aggregate flag alone',()=>{
 assert.deepEqual(bootstrapSelection(list(),input),tuple);
 for(const mutate of [t=>t.state.state='pending',t=>t.capabilities.start=false,t=>t.seats[0].observed_owner=null,t=>t.seats[0].observed_owner.generation=1,t=>t.seats[0].observed_owner.state='starting',t=>t.seats[0].observed_owner.thread_bound=true,t=>t.seats[0].observed_owner.process_count=1,t=>t.seats[0].observed_owner.actual=tuple,t=>t.seats[0].observed_owner.configured={...tuple,model:'other'},t=>t.snapshot.seats=[{...seat,harness:'claude'}],t=>t.snapshot.seats=[],t=>t.seats.push(t.seats[0])]) {
  const v=list();mutate(v.result[0]);assert.throws(()=>bootstrapSelection(v,input));
 }
 assert.throws(()=>parseTeamList({action:'list_pending',result:[]}));
});
test('success receipt requires exact active observed tuple, bound process/thread and no blockers',()=>{
 assert.equal(parseBootstrapStarted(started(),input,tuple).result.phase,'started');
 for(const mutate of [r=>r.seat='other',r=>r.team='other',r=>r.generation=2,r=>r.phase='starting',r=>r.owner.state='stale',r=>r.owner.actual=null,r=>r.owner.actual={...tuple,model:'other'},r=>r.owner.configured={...tuple,reasoning:'low'},r=>r.owner.thread_bound=false,r=>r.owner.process_count=0,r=>r.blockers=[{code:'blocked'}]]) {
  const v=started();mutate(v.result);assert.throws(()=>parseBootstrapStarted(v,input,tuple),/E_CONTROL_UNKNOWN/);
 }
});
test('runtime project keys support repository names without a compiled project allowlist',()=>{
 for(const v of ['project:eunenem-engine','project:quiz-incluir','project:aperture']) assert.equal(projectLabelSchema.parse(v),v);
 for(const v of ['eunenem-engine','project:','project:../x','project:UPPER','project:x/y','project:a\n','project:'+ 'a'.repeat(65)]) assert.equal(projectLabelSchema.safeParse(v).success,false);
 assert.equal(saveRepositorySchema.safeParse({project:'project:eunenem-engine',repo:'eunenem-engine',display_name:'EuNeném',enabled:true,expected_sha256:'a'.repeat(64)}).success,true);
 assert.equal(createTeamSchema.safeParse({team:'mural',project:'project:eunenem-engine',repo:'eunenem-engine',mission:'mission',acceptance:'tests',preset_id:null,seats:[{role:'frontend',...tuple},{role:'qa',...tuple}],lead_index:0,fallbacks:[]}).success,true);
});

test('bootstrap receipt matches the actual Rust-serialized shared wire fixture',()=>{
 const wire=JSON.parse(readFileSync(new URL('../../tests/fixtures/team-bootstrap-response.json',import.meta.url),'utf8'));
 assert.equal(parseBootstrapStarted(wire,input,tuple).result.owner.actual.model,'gpt-5.6-sol');
});

test('normal Claude admission is exact Sonnet/None, still requires native capability and actual observation',()=>{
 const claude={harness:'claude',model:'claude-sonnet-5',reasoning:null};
 function claudeList(t=claude){
  const v=list(), row=v.result[0], s={...seat,...t};
  row.snapshot.seats=[s]; row.seats[0].configured=s;
  row.seats[0].observed_owner.configured=t;
  return v;
 }
 assert.deepEqual(bootstrapSelection(claudeList(),input),claude);
 assert.deepEqual([...CLAUDE_EXACT_MODELS],['claude-sonnet-5','claude-fable-5-1','claude-opus-5']);
 for(const model of CLAUDE_EXACT_MODELS){const t={...claude,model};assert.deepEqual(bootstrapSelection(claudeList(t),input),t);assert.throws(()=>bootstrapSelection(claudeList({...t,reasoning:'low'}),input));}
 for(const t of [{...claude,model:'sonnet'},{...claude,model:'fable'},{...claude,model:'claude-other'},{...claude,model:'claude-fable-5'},{...claude,model:'claude-opus-5-5'},{...claude,model:'claude-fable-5-1[1m]'},{...claude,reasoning:'high'}])
  assert.throws(()=>bootstrapSelection(claudeList(t),input));
 const disabled=claudeList();disabled.result[0].capabilities.start=false;
 assert.throws(()=>bootstrapSelection(disabled,input));
 const wire=started();wire.result.owner.configured=claude;wire.result.owner.actual=claude;
 assert.deepEqual(parseBootstrapStarted(wire,input,claude).result.owner.actual,claude);
 wire.result.owner.actual=null;
 assert.throws(()=>parseBootstrapStarted(wire,input,claude),/E_CONTROL_UNKNOWN/);
});
