import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {spawnSync} from 'node:child_process';
const root=process.env.E3_EXPORT_ROOT;
const receipt=process.env.E3_TEST_EVIDENCE;
const sha=p=>crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
function inventory(dir) {
 const rows=[];let bytes=0;
 function walk(relative) {
  for(const name of fs.readdirSync(path.join(dir,relative)).sort()) {
   const rel=path.join(relative,name),p=path.join(dir,rel),s=fs.lstatSync(p);
   const row={path:rel,mode:s.mode&0o7777,size:s.size,nlink:s.nlink,kind:s.isSymbolicLink()?'symlink':s.isDirectory()?'directory':'file'};
   if(row.kind==='file'){row.sha256=sha(p);bytes+=s.size;}
   if(row.kind==='symlink')row.target=fs.readlinkSync(p);
   rows.push(row);if(row.kind==='directory')walk(rel);
  }
 }
 walk('');return {rows,files:rows.filter(x=>x.kind==='file').length,entries:rows.length,bytes};
}
test('Sentry declared start matches the real tsc layout',()=>{
 const pkg=JSON.parse(fs.readFileSync(new URL('../mcp-server-sentry/package.json',import.meta.url),'utf8'));
 assert.equal(pkg.scripts.start,'node dist/src/index.js');
});
test('native export has regular relative shims, no symlinks or external shim paths', {skip:!root},()=>{
 const data=inventory(root);
 assert.equal(data.rows.filter(x=>x.kind==='symlink').length,0);
 const bins=fs.readdirSync(path.join(root,'node_modules/.bin')).sort();
 assert.deepEqual(bins,['acorn','js-yaml','node-which','sentry-mcp']);
 for(const name of bins){
  const p=path.join(root,'node_modules/.bin',name),s=fs.lstatSync(p);assert.ok(s.isFile());assert.equal(s.nlink,1);
  const text=fs.readFileSync(p,'utf8');assert.ok(text.includes('$basedir/../'));assert.ok(!text.includes('NODE_PATH'));
  assert.ok(!text.includes(root));assert.ok(!text.includes(process.env.TMPDIR));
 }
 assert.ok(fs.lstatSync(path.join(root,'dist/src/index.js')).isFile());
 const prepared=path.join(process.env.TMPDIR,'sentry-prepare');
 for(const file of ['package.json','pnpm-lock.yaml'])assert.equal(sha(path.join(root,file)),sha(path.join(prepared,file)));
 // E1 limits are measured, not relaxed or converted into a passing release gate.
 data.e1={fileCap:8192,entryCap:10000,byteCap:536870912,filesPass:data.files<=8192,entriesPass:data.entries<=10000,bytesPass:data.bytes<=536870912,finalValidation:'NOT_RUN'};
 if(receipt)fs.writeFileSync(path.join(receipt,'raw-export.json'),JSON.stringify(data,null,2));
});
test('export recipe refuses existing destination before install or overwrite', {skip:!root},()=>{
 const before=inventory(root);
 const r=spawnSync(process.env.E3_JUST,['--justfile',path.resolve('justfile'),'e3-sentry-export',process.execPath,process.env.E3_PNPM,path.join(process.env.TMPDIR,'store'),path.join(process.env.TMPDIR,'sentry-prepare'),root],{env:{HOME:process.env.HOME,TMPDIR:process.env.TMPDIR},encoding:'utf8',timeout:10000,maxBuffer:65536});
 assert.equal(r.error,undefined);assert.notEqual(r.status,0);assert.match(r.stderr,/File exists/);assert.deepEqual(inventory(root),before);
 if(receipt)fs.writeFileSync(path.join(receipt,'no-overwrite.json'),JSON.stringify({exit:r.status,signal:r.signal,stdout:r.stdout,stderr:r.stderr,unchanged:true},null,2));
});
