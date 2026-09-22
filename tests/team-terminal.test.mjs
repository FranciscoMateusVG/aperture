import {readFileSync} from "node:fs";
import test from "node:test";
import assert from "node:assert/strict";
import {createServer} from "vite";
import {team,execution} from "./fixtures/team-ui.mjs";
const vite=await createServer({appType:"custom",logLevel:"silent",server:{middlewareMode:true}});
const {canOpenSeat,createTerminalCommands}=await vite.ssrLoadModule("/src/services/team-terminal.ts");await vite.close();
function active(){const t=team("active");t.seats[0].observed_owner={generation:2,state:"active",since:"now",configured:{...execution},actual:{...execution},thread_bound:true,process_count:1};return t}
test("Open sends only team seat and generation selectors, never thread/socket/model",async()=>{const t=active();let captured;await createTerminalCommands(async(cmd,args)=>{captured=[cmd,args];return JSON.parse(readFileSync(new URL("./fixtures/team-terminal-response.json",import.meta.url),"utf8"))}).open(t,"t1-backend");assert.deepEqual(captured,["team_open_seat",{input:{team:"t1",seat:"t1-backend",expected_generation:2}}])});
test("ineligible exact target never invokes even when another seat is active",async()=>{for(const patch of [{state:"stale"},{state:"quarantined"},{state:"starting"},{generation:0},{actual:null},{process_count:0},{thread_bound:false},{actual:{...execution,model:"wrong"}}]){const t=active();Object.assign(t.seats[0].observed_owner,patch);let calls=0;await assert.rejects(createTerminalCommands(async()=>{calls++}).open(t,"t1-backend"));assert.equal(calls,0);assert.equal(canOpenSeat(t,"t1-backend"),false)}});
test("foreign response, stale generation and forged window are rejected without retry",async()=>{for(const patch of [{team:"other"},{seat:"other"},{generation:3},{window_id:"@1;kill"},{thread:"unexpected"}]){let calls=0;await assert.rejects(createTerminalCommands(async()=>{calls++;return{team:"t1",seat:"t1-backend",generation:2,window_id:"@42",...patch}}).open(active(),"t1-backend"));assert.equal(calls,1)}});
