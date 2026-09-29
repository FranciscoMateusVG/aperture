import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';import path from 'node:path';import os from 'node:os';import crypto from 'node:crypto';
import {spawnSync} from 'node:child_process';import {build} from 'vite';
const sha=bytes=>crypto.createHash('sha256').update(bytes).digest('hex');
test('two real Vite builds inventory final bytes and copy-public assets; existing output refused',async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'aperture-ui-builds-'));fs.chmodSync(root,0o700);
 const A=path.join(root,'A'),B=path.join(root,'B'),pub=path.join(root,'public');fs.mkdirSync(pub);
 fs.writeFileSync(path.join(pub,'copied.svg'),'<svg xmlns="http://www.w3.org/2000/svg"/>');
 const just=process.env.APERTURE_TEST_JUST;assert.ok(just?.startsWith('/'),'explicit native just required');
 const first=spawnSync(just,['ui-build',process.execPath,A],{cwd:process.cwd(),env:{HOME:process.env.HOME,TMPDIR:process.env.TMPDIR},encoding:'utf8',timeout:90000,maxBuffer:1048576});
 assert.equal(first.status,0,first.stderr+first.stdout);
 await build({mode:'web-release',publicDir:pub,logLevel:'silent',build:{outDir:B}});
 const manifests=[A,B].map(dir=>{
  const m=JSON.parse(fs.readFileSync(path.join(dir,'UI.json')));assert.match(m.ui_id,/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/);assert.equal(m.api_schema,1);assert.equal(m.schema_version,1);
  assert.deepEqual(m.files.map(f=>f.path),m.files.map(f=>f.path).sort());assert.ok(m.files.length<=512);
  for(const f of m.files){const bytes=fs.readFileSync(path.join(dir,f.path));assert.equal(bytes.length,f.bytes);assert.equal(sha(bytes),f.sha256);assert.ok(!fs.lstatSync(path.join(dir,f.path)).isSymbolicLink());}
  const html=fs.readFileSync(path.join(dir,'index.html'),'utf8');assert.ok(html.includes(`/ui/${m.ui_id}/assets/`));assert.ok(html.includes(m.ui_id));
  return m;
 });
 assert.notEqual(manifests[0].ui_id,manifests[1].ui_id);assert.ok(manifests[1].files.some(f=>f.path==='copied.svg'));
 const before=sha(fs.readFileSync(path.join(A,'UI.json')));
 const denied=spawnSync(just,['ui-build',process.execPath,A],{cwd:process.cwd(),env:{HOME:process.env.HOME,TMPDIR:process.env.TMPDIR},encoding:'utf8',timeout:5000,maxBuffer:1048576});
 assert.notEqual(denied.status,0);assert.equal(sha(fs.readFileSync(path.join(A,'UI.json'))),before);
 assert.ok(process.env.APERTURE_UI_BUILD_RECEIPT,'receipt path required for paired TCP test');
 fs.writeFileSync(process.env.APERTURE_UI_BUILD_RECEIPT,JSON.stringify({A,B,ids:manifests.map(m=>m.ui_id),manifest_hashes:[A,B].map(d=>sha(fs.readFileSync(path.join(d,'UI.json')))),recipe:first,existingDenied:denied},null,2),{flag:'wx',mode:0o600});
});
