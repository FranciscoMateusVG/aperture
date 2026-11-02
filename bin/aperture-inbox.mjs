#!/usr/bin/env node
// aperture-inbox — the receive side of an Aperture agent.
//
// Run by Claude Code's Monitor tool. Prints one line per inbound message on
// stdout (that is how the agent sees it), then acks it to the hub.
//
// It never exits on a network problem: drops, hub restarts and refused
// connections all lead to a reconnect with backoff, then `hello` again, and
// the hub replays anything unacked. It exits only when:
//   * stdout goes away or its parent process dies (the Monitor is gone),
//   * a newer inbox for the same agent connects (hub closes with 4001),
//   * SIGTERM/SIGINT.
//
// Usage: aperture-inbox --agent <name> [--url ws://127.0.0.1:4518/inbox]
// Env:   APERTURE_AGENT, APERTURE_HUB_URL
// Needs Node 22+ (built-in WebSocket). No dependencies.

const args = parseArgs(process.argv.slice(2));
const agent = args.agent ?? process.env.APERTURE_AGENT;
const url = args.url ?? process.env.APERTURE_HUB_URL ?? "ws://127.0.0.1:4518/inbox";
const BACKOFF_MS = [500, 1000, 2000, 4000, 8000, 10000];
const PING_EVERY_MS = 15000;
const DEAD_AFTER_MS = 35000;

if (!agent) {
  process.stderr.write("aperture-inbox: --agent <name> (or APERTURE_AGENT) is required\n");
  process.exit(2);
}

const seen = new Set();
let attempt = 0;
let lastHeard = Date.now();
let ws = null;

process.stdout.on("error", () => process.exit(0)); // Monitor went away
process.on("SIGTERM", () => process.exit(0));
process.on("SIGINT", () => process.exit(0));

function log(line) {
  process.stderr.write(`[aperture-inbox ${agent}] ${line}\n`);
}

function connect() {
  ws = new WebSocket(url);

  ws.onopen = () => {
    attempt = 0;
    lastHeard = Date.now();
    ws.send(JSON.stringify({ type: "hello", agent }));
  };

  ws.onmessage = (event) => {
    lastHeard = Date.now();
    let frame;
    try {
      frame = JSON.parse(event.data);
    } catch {
      return;
    }

    if (frame.type === "welcome") {
      log("connected");
    } else if (frame.type === "message") {
      if (!seen.has(frame.id)) {
        seen.add(frame.id);
        process.stdout.write(formatLines(frame.id, frame.from, String(frame.body)));
      }
      ws.send(JSON.stringify({ type: "ack", id: frame.id }));
    } else if (frame.type === "error") {
      log(`hub error: ${frame.reason}`);
    }
  };

  ws.onclose = (event) => {
    if (event.code === 4001) {
      log("replaced by a newer inbox for this agent, exiting");
      process.exit(0);
    }
    scheduleReconnect();
  };
  ws.onerror = () => {}; // onclose follows
}

// Claude Code's Monitor cuts every stdout line at 500 UTF-16 units and
// appends "...(truncated)". A body that does not fit on one line is split
// over several, each `[aperture] message #N (part i/n) from X: ...`, all
// written at once so they arrive together. Newlines are escaped as \n and an
// escape or a surrogate pair is never split.
const MAX_LINE = 480;

function formatLines(id, from, rawBody) {
  const body = rawBody.replace(/\r?\n/g, "\\n");
  const single = `[aperture] message #${id} from ${from}: ${body}`;
  if (single.length <= MAX_LINE) return single + "\n";

  // tokens: "\n" escapes or single code points
  const tokens = body.match(/\\n|[\s\S]/gu) ?? [];
  // generous prefix budget: assumes the part counter has the most digits n can have
  const prefix = (i, n) => `[aperture] message #${id} (part ${i}/${n}) from ${from}: `;
  let n = 1;
  for (;;) {
    const budget = MAX_LINE - prefix(n, n).length;
    const chunks = [];
    let cur = "";
    for (const t of tokens) {
      if (cur.length + t.length > budget) {
        chunks.push(cur);
        cur = "";
      }
      cur += t;
    }
    chunks.push(cur);
    if (String(chunks.length).length <= String(n).length) {
      return chunks.map((c, i) => prefix(i + 1, chunks.length) + c + "\n").join("");
    }
    n = chunks.length; // more parts than digits budgeted for; redo with a wider counter
  }
}

function scheduleReconnect() {
  const delay = BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)];
  attempt += 1;
  ws = null;
  setTimeout(connect, delay);
}

// Orphan check: if whatever started us (the Monitor, or a version-manager
// shim like Volta) dies, we get re-parented. Exit rather than linger and
// compete with the next inbox for this agent.
const parentPid = process.ppid;
setInterval(() => {
  if (process.ppid !== parentPid) {
    log("parent process gone, exiting");
    process.exit(0);
  }
}, 2000);

// Heartbeat: ping regularly; if the hub has been silent too long, force a reconnect.
setInterval(() => {
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  if (Date.now() - lastHeard > DEAD_AFTER_MS) {
    log("hub silent, reconnecting");
    ws.close();
    return;
  }
  ws.send(JSON.stringify({ type: "ping" }));
}, PING_EVERY_MS);

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i].startsWith("--")) out[argv[i].slice(2)] = argv[i + 1], i++;
  }
  return out;
}

connect();
