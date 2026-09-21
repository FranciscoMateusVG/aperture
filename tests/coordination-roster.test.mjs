import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'vite';

// Same node:test + Vite/fake-DOM layer as the existing component tests.
class Element {
  children = []; dataset = {}; handlers = {}; className = ''; style = { setProperty() {} };
  classList = { add() {}, remove() {} }; selectors = new Map();
  set innerHTML(value) { this.html = value; this.children = []; }
  get innerHTML() { return this.html ?? ''; }
  appendChild(child) { this.children.push(child); return child; }
  setAttribute() {}
  addEventListener(event, handler) { this.handlers[event] = handler; }
  querySelector(selector) {
    if (!this.selectors.has(selector)) this.selectors.set(selector, new Element());
    return this.selectors.get(selector);
  }
}
const tick = () => new Promise(resolve => setImmediate(resolve));
test('actual coordination component renders and bulk-starts/stops only the fixed trio', async () => {
  const priorDocument = globalThis.document, priorRaf = globalThis.requestAnimationFrame;
  globalThis.document = { createElement: () => new Element(), body: new Element(), addEventListener() {} };
  globalThis.requestAnimationFrame = callback => { callback(); return 0; };
  const vite = await createServer({ appType: 'custom', logLevel: 'silent', server: { middlewareMode: true } });
  const { commands } = await vite.ssrLoadModule('/src/services/tauri-commands.ts');
  const saved = { ...commands };
  try {
    const { createAgentList } = await vite.ssrLoadModule('/src/components/AgentList.ts');
    const names = ['glados', 'wheatley', 'peppy', 'rex', 'izzy', 'mission-backend'];
    const agents = names.map(name => ({ name, model: 'codex/gpt-6-astra', role: 'fixture', status: 'stopped', attention: false, tmux_window_id: null }));
    const starts = [], stops = [];
    commands.listAgents = async () => agents.map(agent => ({ ...agent }));
    commands.startAgent = async name => { starts.push(name); agents.find(a => a.name === name).status = 'running'; };
    commands.stopAgent = async name => { stops.push(name); agents.find(a => a.name === name).status = 'stopped'; };
    const container = new Element(); const instance = createAgentList(container); await tick();
    instance.setTeamSeats(['mission-backend']);
    const wrapper = container.children[1];
    const visible = () => wrapper.children.filter(c => c.dataset.agentName).map(c => c.dataset.agentName).sort();
    assert.deepEqual(visible(), ['glados', 'peppy', 'wheatley']);
    wrapper.children[0].children[0].handlers.click(); await tick(); await tick();
    assert.deepEqual(starts.sort(), ['glados', 'peppy', 'wheatley']);
    wrapper.children[0].children[0].handlers.click(); await tick(); await tick();
    assert.deepEqual(stops.sort(), ['glados', 'peppy', 'wheatley']);
    assert.deepEqual(visible(), ['glados', 'peppy', 'wheatley']);
  } finally {
    Object.assign(commands, saved); await vite.close();
    globalThis.document = priorDocument; globalThis.requestAnimationFrame = priorRaf;
  }
});
