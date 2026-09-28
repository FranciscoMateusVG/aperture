// Runs the actual built wrapper against a local inert upstream in a PRIVATE COPY.
// E3_EXPORT_ROOT is test input only; production has no override for upstream selection.
import { test, expect } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import { once } from "node:events";
import type { ChildProcess } from "node:child_process";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const exported = process.env.E3_EXPORT_ROOT;
const logRoot = process.env.E3_TEST_EVIDENCE;
const builtDist = process.env.E3_DIST_ROOT;
const token = "synthetic-e3-not-a-provider-token";
const upstream = `
import fs from 'node:fs';
import path from 'node:path';
import {McpServer} from '@modelcontextprotocol/sdk/server/mcp.js';
import {StdioServerTransport} from '@modelcontextprotocol/sdk/server/stdio.js';
import {z} from 'zod';
const record = value => fs.appendFileSync(path.join(process.env.HOME,'upstream.jsonl'), JSON.stringify(value)+'\\n');
const cf = process.env.__CF_USER_TEXT_ENCODING;
const cfComponent = '(?:[0-9]+|0x[0-9a-fA-F]+)';
const cfEncoding = {present:cf !== undefined, valid:cf !== undefined && Buffer.byteLength(cf,'utf8') <= 64 &&
  new RegExp('^'+cfComponent+':'+cfComponent+':'+cfComponent+'$').exec(cf)?.[0] === cf};
record({kind:'start',pid:process.pid,argv:process.argv,envKeys:Object.keys(process.env).sort(),cfEncoding,
  tokenMatches:process.env.SENTRY_ACCESS_TOKEN==='synthetic-e3-not-a-provider-token',
  masks:Object.fromEntries(['PATH','LOGNAME','SHELL','TERM','USER'].map(k=>[k,process.env[k]])),
  sentinel:process.env.E3_AMBIENT_SENTINEL ?? null, override:process.env.SENTRY_MCP_UPSTREAM_CMD ?? null});
process.stderr.write('synthetic-e3-');process.stderr.write('not-a-provider-token');
const server = new McpServer({name:'inert-local-upstream',version:'1.0.0'});
for(const name of ['search_issues','update_issue']) server.registerTool(name, {inputSchema:{project:z.string()}}, async args=>{
 record({kind:'call',name,args}); return {content:[{type:'text',text:'synthetic-result'}]};
});
const transport = new StdioServerTransport();
await server.connect(transport);
process.stdin.once('end',()=>{void server.close().then(()=>record({kind:'closed',pid:process.pid}));});
process.on('exit',code=>record({kind:'exit',code,pid:process.pid}));
`;

function gone(pid: number) {
  try { process.kill(pid, 0); return false; }
  catch (e) { if ((e as NodeJS.ErrnoException).code === "ESRCH") return true; throw e; }
}
async function bounded<T>(promise: Promise<T>, ms=6000): Promise<T> {
  let timer: NodeJS.Timeout;
  try { return await Promise.race([promise,new Promise<T>((_,reject)=>{timer=setTimeout(()=>reject(new Error("fixture deadline")),ms);})]); }
  finally { clearTimeout(timer!); }
}

test.skipIf(!exported)("real packaged wrapper selects local CLI, masks env, applies policy, closes children", async () => {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),"e3-wrapper-"));
  fs.chmodSync(root,0o700);
  const copy=path.join(root,"package");
  fs.cpSync(exported!,copy,{recursive:true,dereference:false,errorOnExist:true,force:false});
  if(builtDist) fs.cpSync(builtDist,path.join(copy,"dist"),{recursive:true});
  const home=path.join(root,"home");fs.mkdirSync(home,{mode:0o700});
  const allow=path.join(home,"allowlist.yaml");
  fs.writeFileSync(allow,"project_allowlist: [synthetic-project]\nagent_default_on: [aperture-web-backend]\nagent_opt_in: []\n",{mode:0o600});
  const cli=path.join(copy,"node_modules/@sentry/mcp-server/dist/index.js");
  expect(fs.lstatSync(cli).isFile()).toBe(true);
  fs.writeFileSync(cli,upstream); // Only the owned private copy, never the raw export.
  const transport=new StdioClientTransport({command:process.execPath,args:[path.join(copy,"dist/src/index.js")],stderr:"pipe",env:{
    HOME:home, AGENT_NAME:"aperture-web-backend", SENTRY_ACCESS_TOKEN:token,
    SENTRY_MCP_ALLOWLIST_PATH:allow, LOKI_URL:"disabled:",
    E3_AMBIENT_SENTINEL:"must-not-reach-upstream", PATH:"/__e3_no_tools__", SHELL:"e3-shell-sentinel",USER:"e3-user-sentinel",LOGNAME:"e3-logname-sentinel",TERM:"e3-term-sentinel",
    SENTRY_MCP_UPSTREAM_CMD:"/__never_execute__",SENTRY_MCP_UPSTREAM_ARGS:"must not be parsed"
  }});
  let stderr="";
  let markReady!:()=>void; const ready=new Promise<void>(resolve=>{markReady=resolve;});
  transport.stderr!.on("data",chunk=>{stderr+=String(chunk);if(stderr.includes("server ready on stdio"))markReady();if(stderr.length>65536)throw new Error("fixture stderr cap");});
  const client=new Client({name:"e3-wrapper-test",version:"1.0.0"});
  let child:ChildProcess|undefined; let exited:Promise<unknown[]>|undefined;
  let ledger: any[]=[];
  try {
    const connecting=client.connect(transport,{timeout:5000});
    // Observation only of the pinned SDK child; no alternative transport/spawn path.
    child=(transport as unknown as {_process?:ChildProcess})._process;
    expect(child?.pid).toBeTypeOf("number"); exited=once(child!,"exit");
    await bounded(connecting);
    await bounded(ready);
    expect(client.getServerVersion()?.name).toBe("aperture-sentry");
    const listed=await client.listTools({}, {timeout:3000});
    expect(listed.tools.map(t=>t.name).sort()).toEqual(["search_issues","update_issue"]);
    const read=await client.callTool({name:"search_issues",arguments:{params:{project:"synthetic-project"}}},undefined,{timeout:3000});
    expect(read.isError).not.toBe(true);expect(JSON.stringify(read)).toContain("synthetic-result");expect(JSON.stringify(read)).not.toContain(token);
    const denied=await client.callTool({name:"update_issue",arguments:{params:{project:"forbidden-project"}}},undefined,{timeout:3000});
    expect(denied.isError).toBe(true);
    ledger=fs.readFileSync(path.join(home,"upstream.jsonl"),"utf8").trim().split("\n").map(line=>JSON.parse(line));
    expect(ledger.filter(row=>row.kind==="call")).toEqual([{kind:"call",name:"search_issues",args:{project:"synthetic-project"}}]);
    const start=ledger[0];expect(start.argv).toEqual([process.execPath,cli]);expect(start.tokenMatches).toBe(true);
    expect(start.sentinel).toBeNull();expect(start.override).toBeNull();
    expect(start.masks).toEqual({PATH:"",LOGNAME:"",SHELL:"",TERM:"",USER:""});
    const expectedKeys = ["HOME","LOGNAME","PATH","SENTRY_ACCESS_TOKEN","SHELL","TERM","USER"];
    expect(start.cfEncoding.present).toBe(start.envKeys.includes("__CF_USER_TEXT_ENCODING"));
    expect(typeof start.cfEncoding.valid).toBe("boolean");
    // Apple CF/CFRuntime.c documents this one Darwin bootstrap key. This narrow
    // format allowance is compatibility, NOT proof of who inserted it here.
    if (process.platform === "darwin" && start.cfEncoding.present) {
      expect(start.cfEncoding.valid).toBe(true);
      expectedKeys.push("__CF_USER_TEXT_ENCODING");
    } else {
      expect(start.cfEncoding.present).toBe(false);
    }
    expect(start.envKeys).toEqual(expectedKeys.sort());
  } finally {
    await bounded(client.close());await bounded(transport.close());
    if(exited)expect(await bounded(exited)).toEqual([0,null]);
    if(child?.pid)expect(gone(child.pid)).toBe(true);
    ledger=fs.existsSync(path.join(home,"upstream.jsonl"))?fs.readFileSync(path.join(home,"upstream.jsonl"),"utf8").trim().split("\n").map(line=>JSON.parse(line)):[];
    const start=ledger.find(row=>row.kind==="start");
    if(start) {expect(ledger.some(row=>row.kind==="closed")).toBe(true);expect(ledger.some(row=>row.kind==="exit"&&row.code===0)).toBe(true);expect(gone(start.pid)).toBe(true);}
    if(logRoot)fs.writeFileSync(path.join(logRoot,"wrapper-receipt.json"),JSON.stringify({root,wrapperPid:child?.pid,ledger,stderr,cleanup:"own children exited and ESRCH; private copy retained"},null,2));
  }
  expect(stderr).not.toContain(token);
},20000);

// The same native SDK stdio channel, with fixture-owned file barriers. No sleep,
// custom JSON-RPC reader, process census or production override is involved.
function barrierUpstream(mode: "initialize" | "list" | "failure") {
  return `
import fs from 'node:fs';
import path from 'node:path';
import {Server} from '@modelcontextprotocol/sdk/server/index.js';
import {StdioServerTransport} from '@modelcontextprotocol/sdk/server/stdio.js';
import {InitializeRequestSchema,ListToolsRequestSchema,CallToolRequestSchema} from '@modelcontextprotocol/sdk/types.js';
const home=process.env.HOME, mode=${JSON.stringify(mode)};
const record=value=>fs.appendFileSync(path.join(home,'upstream.jsonl'),JSON.stringify(value)+'\\n');
record({kind:'start',pid:process.pid});
let release;
function barrier(phase) {
 return new Promise((resolve,reject)=>{
  let done=false;
  const finish=reason=>{if(done)return;done=true;watcher.close();clearTimeout(timer);record({kind:'released',phase,reason});resolve();};
  const check=()=>{if(fs.existsSync(path.join(home,'release')))finish('fixture');};
  const watcher=fs.watch(home,check);
  const timer=setTimeout(()=>{if(done)return;done=true;watcher.close();record({kind:'barrier-timeout',phase});reject(new Error('fixture barrier deadline'));},5000);
  release=()=>finish('caller-eof');
  record({kind:'barrier',phase,pid:process.pid});check();
 });
}
const server=new Server({name:'inert-barrier',version:'1.0.0'},{capabilities:{tools:{}}});
server.setRequestHandler(InitializeRequestSchema,async request=>{
 if(mode==='failure'){record({kind:'startup-failure'});throw new Error('synthetic startup failure');}
 if(mode==='initialize')await barrier('initialize');
 return {protocolVersion:request.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'inert-barrier',version:'1.0.0'}};
});
server.setRequestHandler(ListToolsRequestSchema,async()=>{
 if(mode==='list')await barrier('list');
 return {tools:[{name:'search_issues',inputSchema:{type:'object',properties:{project:{type:'string'}},required:['project']}}]};
});
server.setRequestHandler(CallToolRequestSchema,async()=>{record({kind:'call'});return {content:[{type:'text',text:'inert'}]};});
process.stdin.once('end',()=>{release?.();void server.close().then(()=>record({kind:'closed',pid:process.pid}));});
process.on('exit',code=>record({kind:'exit',pid:process.pid,code}));
await server.connect(new StdioServerTransport());
`;
}

function ledgerAt(home: string): any[] {
  const file=path.join(home,"upstream.jsonl");
  if(!fs.existsSync(file))return [];
  // Only complete append records; a pending partial line is not success.
  return fs.readFileSync(file,"utf8").split("\n").slice(0,-1).map(line=>JSON.parse(line));
}
function waitRecord(home:string, predicate:(rows:any[])=>boolean):Promise<void> {
  return new Promise((resolve,reject)=>{
    let done=false;
    const finish=(error?:unknown)=>{if(done)return;done=true;watcher.close();clearTimeout(timer);error?reject(error):resolve();};
    const check=()=>{try{if(predicate(ledgerAt(home)))finish();}catch(error){finish(error);}};
    const watcher=fs.watch(home,check);
    const timer=setTimeout(()=>finish(new Error("fixture record deadline")),6000);
    check();
  });
}

for(const scenario of ["initialize","list","failure","early-request"] as const) {
  test.skipIf(!exported)(`real wrapper startup lifecycle: ${scenario}`,async()=>{
    const root=fs.mkdtempSync(path.join(os.tmpdir(),"e3-startup-"));fs.chmodSync(root,0o700);
    const copy=path.join(root,"package"),home=path.join(root,"home");
    fs.cpSync(exported!,copy,{recursive:true,dereference:false,errorOnExist:true,force:false});
    if(builtDist)fs.cpSync(builtDist,path.join(copy,"dist"),{recursive:true});
    fs.mkdirSync(home,{mode:0o700});
    const allow=path.join(home,"allowlist.yaml");
    fs.writeFileSync(allow,"project_allowlist: [synthetic-project]\nagent_default_on: [aperture-web-backend]\nagent_opt_in: []\n",{mode:0o600});
    fs.writeFileSync(path.join(copy,"node_modules/@sentry/mcp-server/dist/index.js"),barrierUpstream(scenario==="early-request"?"initialize":scenario));
    const transport=new StdioClientTransport({command:process.execPath,args:[path.join(copy,"dist/src/index.js")],stderr:"pipe",env:{
      HOME:home,AGENT_NAME:"aperture-web-backend",SENTRY_ACCESS_TOKEN:token,SENTRY_MCP_ALLOWLIST_PATH:allow,LOKI_URL:"disabled:",
      PATH:"",SHELL:"",USER:"",LOGNAME:"",TERM:"",
    }});
    let stderr="";let markReady!:()=>void;
    const ready=new Promise<void>(resolve=>{markReady=resolve;});
    transport.stderr!.on("data",chunk=>{stderr+=String(chunk);if(stderr.length>65536)throw new Error("fixture stderr cap");if(stderr.includes("server ready on stdio"))markReady();});
    const client=new Client({name:"e3-startup-test",version:"1.0.0"});
    const connecting=client.connect(transport,{timeout:5000});
    // Attach a rejection observer immediately; the assertion below still awaits it.
    const connected=connecting.then(()=>({ok:true as const}),error=>({ok:false as const,error}));
    const child=(transport as unknown as {_process?:ChildProcess})._process;
    expect(child?.pid).toBeTypeOf("number");
    const exited=once(child!,"exit");
    let outcome:unknown[]|undefined;
    try {
      if(scenario==="failure") {
        await waitRecord(home,rows=>rows.some(row=>row.kind==="startup-failure"));
        outcome=await bounded(exited);
        expect(outcome).toEqual([1,null]);
      } else {
        await waitRecord(home,rows=>rows.some(row=>row.kind==="barrier"));
        expect(stderr).not.toContain("server ready on stdio");
        if(scenario==="early-request") {
          expect((await bounded(connected)).ok).toBe(true);
          expect(client.getServerVersion()?.name).toBe("aperture-sentry");
          expect(await client.ping({timeout:2000})).toEqual({});
          const pending=await client.listTools({}, {timeout:2000});
          expect(pending.tools.map(tool=>tool.name)).toEqual(["_unavailable"]);
          const unavailable=await client.callTool({name:"_unavailable",arguments:{}},undefined,{timeout:2000});
          expect(unavailable.isError).toBe(true);
          fs.writeFileSync(path.join(home,"release"),"release",{flag:"wx",mode:0o600});
          await bounded(ready);
          expect((await client.listTools({}, {timeout:2000})).tools.map(tool=>tool.name)).toEqual(["search_issues"]);
        }
        // Close the owned caller pipe at the barrier, not a timeout-driven signal.
        child!.stdin!.end();
        outcome=await bounded(exited);
        expect(outcome).toEqual([0,null]);
      }
      if(scenario!=="early-request")expect(stderr).not.toContain("server ready on stdio");
      expect(stderr).not.toContain(token);
    } finally {
      await bounded(client.close());await bounded(transport.close());
      outcome ??= await bounded(exited);
      const ledger=ledgerAt(home),start=ledger.find(row=>row.kind==="start");
      if(logRoot)fs.writeFileSync(path.join(logRoot,`startup-${scenario}.json`),JSON.stringify({scenario,root,wrapperPid:child?.pid,outcome,stderr,ledger},null,2));
      expect(gone(child!.pid!)).toBe(true);
      expect(start?.pid).toBeTypeOf("number");expect(gone(start.pid)).toBe(true);
      expect(ledger.some(row=>row.kind==="closed")).toBe(true);
      expect(ledger.some(row=>row.kind==="exit"&&row.code===0)).toBe(true);
      expect(ledger.filter(row=>row.kind==="call"||row.kind==="barrier-timeout")).toEqual([]);
      if(scenario!=="failure")expect(ledger.filter(row=>row.kind==="released").map(row=>row.reason)).toEqual([scenario==="early-request"?"fixture":"caller-eof"]);
    }
  },20000);
}
