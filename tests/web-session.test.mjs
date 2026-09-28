import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'vite';
const session='s'.repeat(43);
function dom(schema='1',exchange=false) {
 const nodes=[],calls=[],data=new Map(exchange?[]:[['aperture.web.session.v1',session]]),lookups=[];
 const element=tag=>{const n={tag,textContent:'',disabled:false,children:[],setAttribute(){},append(...children){this.children.push(...children)}};nodes.push(n);return n;};
 const navbar=element('nav');
 const win={location:{origin:'http://127.0.0.1:4519',pathname:'/',hash:exchange?'#t='+'e'.repeat(43):''},history:{replaceState(){win.location.hash=''}},sessionStorage:{getItem:k=>data.get(k),setItem:(k,v)=>data.set(k,v),removeItem:k=>data.delete(k)},open(){throw Error('not requested')},fetch:async(p,o)=>{calls.push([p,o]);return new Response(JSON.stringify(p==='/session'?{session}:p==='/session/logout'?{revoked:true}:'v'),{headers:{'X-Aperture-Api-Schema':schema}})}};
 globalThis.window=win;globalThis.document={createElement:element,getElementById:id=>{lookups.push(id);assert.equal(id,'navbar','main init must not execute');return navbar;}};
 return {nodes,calls,data,lookups,win,setSchema(value){schema=value;}};
}
async function vite(){return createServer({configFile:false,appType:'custom',logLevel:'silent',define:{__APERTURE_WEB_BUILD__:JSON.stringify({ui_id:'a'.repeat(32),api_schema:1})},server:{middlewareMode:true}})}
test('actual main ingress refuses incompatible session before init or polling; logout still works',async()=>{
 const v=await vite();const d=dom('2',true);let intervals=0;const old=globalThis.setInterval;globalThis.setInterval=()=>{intervals++;throw Error('polling before gate');};
 try {
  await v.ssrLoadModule('/src/main.ts');await new Promise(setImmediate);
  assert.deepEqual(d.calls.map(c=>c[0]),['/session','/api/version']);assert.equal(intervals,0);assert.deepEqual(d.lookups,['navbar']);
  assert.ok(d.nodes.some(n=>n.textContent==='UI/API incompatible; reload Aperture'));
  const logout=d.nodes.find(n=>n.textContent==='Log out');assert.equal(logout.disabled,false);await logout.onclick();assert.equal(d.calls.at(-1)[0],'/session/logout');assert.equal(d.data.size,0);
 }finally{globalThis.setInterval=old;await v.close();delete globalThis.window;delete globalThis.document;}
});
test('session exchange and refresh resolve only after schema gate; Tauri bypass invokes no web I/O',async()=>{
 const v=await vite();try{
  const {initializeBrowserSession}=await v.ssrLoadModule('/src/services/web-session.ts');
  for(const exchange of [false,true]){const d=dom('1',exchange);assert.equal(await initializeBrowserSession(),true);assert.deepEqual(d.calls.map(c=>c[0]),exchange?['/session','/api/version']:['/api/version']);}
  globalThis.window={__TAURI_INTERNALS__:{}};globalThis.document={createElement(){throw Error('Tauri web gate')}};
  assert.equal(await initializeBrowserSession(),true);
 }finally{await v.close();delete globalThis.window;delete globalThis.document;}
});

test('late API mismatch keeps reload required sticky on link while logout remains available',async()=>{
 const v=await vite();const d=dom();let opened=0;d.win.open=()=>{opened++;};
 try {
  const {initializeBrowserSession}=await v.ssrLoadModule('/src/services/web-session.ts');
  assert.equal(await initializeBrowserSession(),true);
  const link=d.nodes.find(n=>n.textContent==='Open another window');
  const logout=d.nodes.find(n=>n.textContent==='Log out');
  const status=d.nodes.find(n=>n.tag==='span');
  d.setSchema('2');await link.onclick();
  assert.equal(status.textContent,'UI/API incompatible; reload Aperture');
  assert.equal(link.disabled,true);assert.equal(logout.disabled,false);
  assert.equal(opened,0);assert.deepEqual(d.calls.map(c=>c[0]),['/api/version','/api/version']);
  await link.onclick();
  assert.equal(opened,0);assert.equal(d.calls.length,2,'no mint or automatic/user retry after mismatch');
  assert.equal(status.textContent,'UI/API incompatible; reload Aperture');
  await logout.onclick();
  assert.equal(d.calls.at(-1)[0],'/session/logout');assert.equal(d.data.size,0);
  assert.equal(link.disabled,true);assert.equal(logout.disabled,true);
 }finally{await v.close();delete globalThis.window;delete globalThis.document;}
});
