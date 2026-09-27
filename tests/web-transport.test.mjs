import test from "node:test";
import assert from "node:assert/strict";
import {createServer} from "vite";
import {team} from "./fixtures/team-ui.mjs";
const vite=await createServer({appType:"custom",logLevel:"silent",server:{middlewareMode:true}});
const {createWebTransport,commandRoute,takeExchange,SESSION_KEY}=await vite.ssrLoadModule("/src/services/web-transport.ts");
const {createTeamCommands}=await vite.ssrLoadModule("/src/services/team-commands.ts");
const {createCommands}=await vite.ssrLoadModule("/src/services/tauri-commands.ts");
await vite.close();
const session="s".repeat(43),exchange="e".repeat(43);
function fixture(response=()=>new Response(JSON.stringify({semver:"fixture"}),{status:200})) {
 const data=new Map([[SESSION_KEY,session]]),calls=[];let ended=0;
 const storage={getItem:k=>data.get(k)??null,setItem:(k,v)=>data.set(k,v),removeItem:k=>data.delete(k)};
 const api=createWebTransport({storage,onEnded:()=>ended++,fetch:async(path,options)=>{calls.push([path,options]);return response(path,options)}});
 return {api,data,calls,get ended(){return ended}};
}
test("finite mapping covers all 20 registrations without a generic invocation route",()=>{
 const entries=[
 ["get_version",{},"/api/version"],["list_agents",{},"/api/agents"],
 ...["start","stop","restart"].map(v=>[`${v}_agent`,{name:"t1-backend"},`/api/agents/t1-backend/${v}`]),
 ["update_agent_model",{name:"t1-backend",model:"codex/gpt-6-astra"},"/api/agents/t1-backend/model"],
 ["clear_attention",{name:"t1-backend"},"/api/agents/t1-backend/attention/clear"],
 ["tmux_create_session",{sessionName:"aperture"},"/api/tmux/session"],["tmux_select_window",{windowId:"@1"},"/api/tmux/select-window"],
 ["team_get_catalog",{},"/api/teams/catalog"],["team_list_presets",{},"/api/teams/presets"],["team_list",{},"/api/teams"],
 ["team_save_preset",{input:{preset:{}}},"/api/teams/presets"],["team_create",{input:{team:"t1"}},"/api/teams"],
 ["team_cancel_pending",{input:{team:"t1"}},"/api/teams/t1/cancel"],
 ["team_prepare_replacement",{input:{team:"t1"}},"/api/teams/t1/replacement/prepare"],["team_start_replacement",{input:{team:"t1"}},"/api/teams/t1/replacement/start"],
 ["team_archive",{input:{team:"t1"}},"/api/teams/t1/archive"],["team_open_seat",{input:{team:"t1",seat:"t1-backend"}},"/api/teams/t1/seats/t1-backend/open"],
 ["team_bootstrap_seat",{input:{team:"t1",seat:"t1-backend",expected_generation:0}},"/api/teams/t1/seats/t1-backend/bootstrap"],
 ]; assert.equal(entries.length,20);
 for(const [command,args,path] of entries) assert.equal(commandRoute(command,args).path,path);
 for(const command of ["__proto__","invoke","shell","team_approve","team_stop_seat"]) assert.throws(()=>commandRoute(command,{input:{team:"t1"}}));
 assert.throws(()=>commandRoute("start_agent",{name:"../../shell"}));
 assert.deepEqual(commandRoute("tmux_create_session",{sessionName:"aperture"}).body,{session_name:"aperture"});
});
test("adapter composes real service factories; selectors retain snake_case and parsers remain active",async()=>{
 const f=fixture(()=>new Response(JSON.stringify([team()]),{status:200}));
 assert.deepEqual(await createTeamCommands(f.api.call).list(),[team()]);
 assert.equal(f.calls[0][0],"/api/teams");assert.equal(f.calls[0][1].headers.Authorization,`Bearer ${session}`);
 const bad=fixture(()=>new Response(JSON.stringify({wrong:true}),{status:200}));
 await assert.rejects(createTeamCommands(bad.api.call).list(),e=>e.code==="E_RESPONSE_INVALID");
 const write=fixture(()=>new Response("null",{status:200}));await createCommands(write.api.call).tmuxSelectWindow("@9");
 assert.equal(write.calls[0][1].body,JSON.stringify({window_id:"@9"}));
});
test("refresh reuses browsing-context session; exchange is in JSON only, fragment removed synchronously",async()=>{
 const f=fixture(()=>new Response(JSON.stringify({session}),{status:200}));let removed=false;
 assert.equal(takeExchange({hash:`#t=${exchange}`,pathname:"/"},{replaceState(_state,_title,path){removed=true;assert.equal(path,"/")}}),exchange);assert.equal(removed,true);
 await f.api.exchange(exchange);assert.equal(f.data.get(SESSION_KEY),session);
 assert.equal(f.calls[0][0],"/session");assert.equal(f.calls[0][1].headers.Authorization,undefined);assert.equal(JSON.parse(f.calls[0][1].body).exchange,exchange);
 await f.api.resume();assert.equal(f.calls[1][1].headers.Authorization,`Bearer ${session}`);
 assert.equal(f.calls[1][1].credentials,"omit");assert.equal(f.calls[1][1].redirect,"error");
 for(const [path] of f.calls) {assert.ok(!path.includes(session));assert.ok(!path.includes(exchange));}
});
test("non2xx TeamError and lost transport are never mutation retries; 401 clears storage",async()=>{
 const f=fixture(()=>new Response(JSON.stringify({code:"E_WEB_AUTHORITY_DENIED",message:"fixed"}),{status:403}));
 await assert.rejects(f.api.call("team_bootstrap_seat",{input:{team:"t1",seat:"t1-backend"}}),e=>e.code==="E_WEB_AUTHORITY_DENIED");assert.equal(f.calls.length,1);
 const lost=fixture(()=>{throw Error("private transport detail")});await assert.rejects(lost.api.call("stop_agent",{name:"x"}),e=>e.code==="E_WEB_OUTCOME_UNKNOWN"&&!e.message.includes("private"));assert.equal(lost.calls.length,1);
 const denied=fixture(()=>new Response("{}",{status:401}));await assert.rejects(denied.api.resume(),e=>e.code==="E_WEB_SESSION_ENDED");assert.equal(denied.data.has(SESSION_KEY),false);assert.equal(denied.ended,1);
 await assert.rejects(denied.api.resume());assert.equal(denied.calls.length,1);
});
test("logout revokes before client removal; link never returns a session bearer",async()=>{
 const f=fixture((path)=>new Response(JSON.stringify(path==="/session/link"?{exchange}:{revoked:true}),{status:200}));
 assert.equal(await f.api.link(),`http://127.0.0.1:4519/#t=${exchange}`);assert.equal(f.data.get(SESSION_KEY),session);
 await f.api.logout();assert.equal(f.calls.at(-1)[0],"/session/logout");assert.equal(f.data.has(SESSION_KEY),false);
 const failed=fixture(()=>new Response("{}",{status:503}));await assert.rejects(failed.api.logout());assert.equal(failed.data.get(SESSION_KEY),session);
});
test("all nine legacy service methods use the injected call and unknown authority args are refused",async()=>{
 const calls=[];const api=createCommands(async(command,args)=>{calls.push([command,args]);return null});
 await api.startAgent('a');await api.stopAgent('a');await api.restartAgent('a');await api.listAgents();await api.updateAgentModel('a','sonnet');await api.clearAttention('a');await api.getVersion();await api.tmuxCreateSession('aperture');await api.tmuxSelectWindow('@1');
 assert.equal(calls.length,9);assert.equal(new Set(calls.map(x=>x[0])).size,9);
 for(const [command,args] of [['tmux_create_session',{sessionName:'aperture',actor:'glados'}],['start_agent',{name:'a',principal:'glados'}],['get_version',{capability:'x'}]]) assert.throws(()=>commandRoute(command,args));
});
