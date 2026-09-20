// Handler-contract fixture using the existing FakeElement/Vite SSR approach.
// It does NOT implement layout, tab order or a native modal focus trap.
import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "vite";
import { preset, catalog, clone, created } from "./fixtures/team-ui.mjs";
const vite = await createServer({ appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
const { openTeamEditor } = await vite.ssrLoadModule("/src/components/TeamEditor.ts"); await vite.close();
const decode = s => s.replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");
function fixture() {
 const named = new Map(), ids = new Map(), controls = [];
 class Element {
  attrs = {}; dataset = {}; value = ""; textContent = ""; disabled = false; isConnected = true; listeners = {}; name = "";
  setAttribute(k, v) { this.attrs[k] = v; } removeAttribute(k) { delete this.attrs[k]; }
  addEventListener(type, fn) { (this.listeners[type] ??= []).push(fn); }
  async fire(type, event = {}) { for (const fn of this.listeners[type] ?? []) await fn(event); }
  focus() { document.activeElement = this; }
  closest() { return this; }
 }
 function parse(html, prefix) {
  if (prefix) for (const name of named.keys()) if (prefix.test(name)) named.delete(name);
  for (const match of html.matchAll(/<input\b([^>]*)>|<select\b([^>]*)>([\s\S]*?)<\/select>/g)) {
   const attrs = match[1] ?? match[2], name = /\bname="([^"]+)"/.exec(attrs)?.[1]; if (!name) continue;
   const el = new Element(); el.name = name;
   el.value = decode(/\bvalue="([^"]*)"/.exec(attrs)?.[1] ?? "");
   if (match[2] !== undefined) {
    const opts = [...(match[3] ?? "").matchAll(/<option value="([^"]*)"([^>]*)>/g)];
    el.value = decode((opts.find(x => /selected/.test(x[2])) ?? opts[0])?.[1] ?? "");
   }
   if (name !== "lead" || /\bchecked\b/.test(attrs) || !named.has(name)) named.set(name, el);
   if (name === "lead" && !/checked/.test(attrs) && !named.has("lead-checked")) { /* radio group fixed below */ }
   const id = /\bid="([^"]+)"/.exec(attrs)?.[1]; if (id) ids.set(id, el);
   controls.push(el);
  }
  if (prefix?.test("lead") && !/name="lead"[^>]*checked/.test(html)) named.set("lead", Object.assign(new Element(), { value: "" }));
 }
 const origin = new Element(), errors = new Element(), status = new Element(), addSeat = new Element(), addFallback = new Element();
 const previews = [];
 const seats = new Element(); Object.defineProperty(seats, "innerHTML", { set(html) { parse(html, /^(role-|execution-|lead)/); previews.length = 0; for (const _ of html.matchAll(/data-seat-name/g)) previews.push(new Element()); }, get() { return ""; } });
 seats.querySelectorAll = () => previews;
 seats.querySelector = selector => named.get(/name="([^"]+)"/.exec(selector)?.[1]) ?? null;
 const fallbacks = new Element(); Object.defineProperty(fallbacks, "innerHTML", { set(html) { parse(html, /^fallback-/); } });
 fallbacks.querySelector = seats.querySelector;
 const form = new Element(); form.elements = { namedItem: name => named.get(name) ?? null };
 form.querySelectorAll = selector => selector === "input,button,select" ? controls : selector === "[aria-invalid]" ? [...named.values()].filter(e => "aria-invalid" in e.attrs) : [];
 form.querySelector = selector => selector === '[aria-invalid="true"]' ? [...named.values()].find(e => e.attrs["aria-invalid"] === "true") : selector.includes("add-seat") ? addSeat : addFallback;
 const dialog = new Element();
 Object.defineProperty(dialog, "innerHTML", { set(html) { parse(html); dialog.html = html; } });
 dialog.querySelector = selector => ({ form, ".v4-seats": seats, ".v4-fallbacks": fallbacks, "[data-status]": status, "[data-errors]": errors })[selector];
 dialog.contains = () => true; dialog.showModal = () => { dialog.open = true; };
 dialog.remove = () => { dialog.isConnected = false; };
 dialog.close = () => { dialog.open = false; void dialog.fire("close"); };
 const document = { activeElement: origin, createElement: () => dialog, getElementById: id => ids.get(id) ?? null, body: { appendChild() {} } };
 const prior = { document: globalThis.document, HTMLElement: globalThis.HTMLElement };
 globalThis.document = document; globalThis.HTMLElement = Element;
 return {
  dialog, form, named, errors, status, document, origin, previews,
  restore() { for (const [key, value] of Object.entries(prior)) if (value === undefined) delete globalThis[key]; else globalThis[key] = value; },
  input(name, value) { named.get(name).value = value; return form.fire("input", { target: named.get(name) }); },
  change: () => form.fire("change"),
  click(action, index) { const button = new Element(); button.dataset = { action, index: String(index) }; return form.fire("click", { target: button }); },
  submit() { return form.fire("submit", { preventDefault() {} }); },
  escape() { const ev = { prevented: false, preventDefault() { this.prevented = true; } }; return dialog.fire("cancel", ev).then(() => { if (!ev.prevented) dialog.close(); return ev; }); },
 };
}
async function run(options, fn) {
 const f = fixture();
 try { openTeamEditor({ mode: "create", preset: clone(preset), catalog: clone(catalog), knownSeats: [], knownTeams: [], submitTeam: async () => created(), submitPreset: async () => preset, saved() {}, ...options }); await fn(f); }
 finally { f.restore(); }
}
test("actual handlers preserve editing/paste/delete and reject invalid team before submit", async () => {
 let calls = 0;
 await run({ submitTeam: async () => { calls++; return created(); } }, async f => {
  await f.input("team", "T1.x"); await f.submit(); assert.equal(calls, 0); assert.match(f.errors.textContent, /lowercase/); assert.equal(f.named.get("team").attrs["aria-invalid"], "true");
  await f.input("team", ""); await f.submit(); assert.equal(calls, 0);
  await f.input("team", "t1"); await f.input("mission", " Pasted mission "); await f.change();
  assert.equal(f.named.get("mission").value, " Pasted mission "); assert.equal(f.previews[0].textContent, "t1-backend");
 });
});
test("submit exact detached values; duplicate click locked; Escape blocked while pending, then focus returns", async () => {
 let resolve, sent, calls = 0, saved = 0;
 await run({ submitTeam: input => { calls++; sent = input; return new Promise(r => { resolve = r; }); }, saved: () => saved++ }, async f => {
  await f.input("team", "t1"); await f.input("mission", "  Exact mission  ");
  const pending = f.submit(); assert.equal(calls, 1); assert.equal(f.form.attrs["aria-busy"], "true"); assert.match(f.status.textContent, /No success/);
  await f.submit(); assert.equal(calls, 1); assert.equal((await f.escape()).prevented, true);
  assert.equal(sent.mission, "  Exact mission  "); assert.equal(sent.seats[0].model, "gpt-6-astra"); assert.equal(saved, 0);
  resolve(created()); await pending; assert.equal(saved, 1); assert.equal(f.dialog.open, false); assert.equal(f.document.activeElement, f.origin);
 });
});
test("failure retains draft and supports explicit retry without auto retry", async () => {
 let calls = 0;
 await run({ submitTeam: async () => { calls++; throw { code: "E_LOCK_HELD", message: "SENTINEL_SECRET" }; } }, async f => {
  await f.input("team", "t1"); await f.submit(); assert.equal(calls, 1); assert.equal(f.dialog.open, true); assert.equal(f.named.get("team").value, "t1");
  assert.match(f.errors.textContent, /Another operation/); assert.equal(f.errors.textContent.includes("SENTINEL_SECRET"), false); assert.equal(f.form.attrs["aria-busy"], "false");
  await f.submit(); assert.equal(calls, 2); await f.click("cancel"); assert.equal(f.document.activeElement, f.origin);
 });
});
test("removing lead demands a new explicit selection; repeated role preview derives suffix", async () => {
 await run({}, async f => {
  await f.input("team", "t1"); await f.click("add-seat"); f.named.get("role-2").value = "backend"; f.named.get("execution-2").value = "0"; await f.change();
  assert.equal(f.previews[2].textContent, "t1-backend-2"); await f.click("remove-seat", 0); await f.submit(); assert.match(f.errors.textContent, /exactly one lead/);
 });
});
test("edit uses exact CAS; duplicate uses new id and null CAS without changing source preset", async () => {
 for (const mode of ["edit", "duplicate"]) {
  let sent;
  await run({ mode, submitPreset: async (...args) => { sent = args; return preset; } }, async f => {
   if (mode === "duplicate") await f.input("presetId", "fullstack-copy");
   await f.submit(); assert.ok(sent); assert.equal(sent[1], mode === "edit" ? preset.sha256 : null);
   assert.equal(sent[0].id, mode === "edit" ? "fullstack" : "fullstack-copy"); assert.equal("source" in sent[0], false); assert.equal("sha256" in sent[0], false);
  });
 }
});
test("blank preset, cancel/Escape and source accessibility contracts", async () => {
 await run({ mode: "blank", preset: undefined }, async f => {
  assert.match(f.dialog.html, /<label for="v4-presetId">/); assert.match(f.dialog.html, /role="alert"/); assert.match(f.dialog.html, /aria-live="polite"/);
  await f.submit(); assert.match(f.errors.textContent, /1 and 99 seats/); await f.escape(); assert.equal(f.dialog.open, false); assert.equal(f.document.activeElement, f.origin);
 });
});
