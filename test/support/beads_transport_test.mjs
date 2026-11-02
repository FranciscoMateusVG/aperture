import { test } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

for (const mode of ['body-stalls', 'connection-lost']) {
  test(`MCP write ${mode}: unknown result, never retry`, async () => {
    let requests = 0;
    const server = http.createServer((req, res) => {
      requests++;
      req.resume();
      if (mode === 'connection-lost') return req.socket.destroy();
      res.writeHead(200, {'content-type': 'application/json'});
      res.write('{'); // deadline must cover body parsing too, not just headers
      setTimeout(() => req.socket.destroy(), 600);
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    const script = fileURLToPath(new URL('../../bin/aperture-mcp.mjs', import.meta.url));
    const child = spawn(process.execPath, [script, '--agent', 'peppy', '--hub', `http://127.0.0.1:${server.address().port}`],
      {env: {...process.env, APERTURE_BEADS_TIMEOUT_MS: '100'}, stdio: ['pipe', 'pipe', 'pipe']});
    try {
      const started = Date.now();
      let out = '';
      child.stdout.on('data', chunk => {out += chunk;});
      child.stdin.end(JSON.stringify({jsonrpc:'2.0', id:1, method:'tools/call', params:{name:'create_task',arguments:{title:'fixture'}}}) + '\n');
      const code = await new Promise(resolve => child.on('exit', resolve));
      assert.equal(code, 0);
      const reply = JSON.parse(out);
      assert.equal(reply.result.isError, true);
      const detail = JSON.parse(reply.result.content[0].text);
      assert.equal(detail.outcome, 'indeterminate');
      assert.equal(detail.retry_safe, false);
      assert.match(detail.message, /may have committed/i);
      assert.match(detail.message, /do not retry/i);
      assert.equal(requests, 1);
      assert.ok(Date.now() - started < 550, 'deadline covers headers and body');
    } finally {
      child.kill();
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
    }
  });
}
