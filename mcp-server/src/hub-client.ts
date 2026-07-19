/**
 * hub-client — the canonical inbox-monitor client for Claude agents.
 *
 * Agents run this via a bash-based Monitor:
 *
 *   Monitor({
 *     command: "node <repo>/mcp-server/dist/hub-client.js <agent-name>",
 *     persistent: true,
 *   })
 *
 * Why this exists (aperture-1qwty): the Monitor tool's native ws source is
 * RECEIVE-ONLY — it cannot send the hello frame the hub requires to identify
 * the connection. An agent that connects without hello is an anonymous socket:
 * no presence registration, no unread replay, no push delivery. This wrapper
 * sends the hello on open and streams every hub frame as one stdout line
 * (= one Monitor event).
 *
 * Behaviour:
 *   - URL from APERTURE_HUB_URL (default ws://127.0.0.1:4517)
 *   - perMessageDeflate OFF (hub requirement — banked gotcha)
 *   - hello: { type: "hello", role: "agent", agent: <argv[2]> }
 *     plus token: APERTURE_HUB_TOKEN when set (forward-compat with
 *     aperture-278a4 hub auth; harmless extra field until the hub enforces)
 *   - each incoming frame → one stdout line
 *   - close/error → diagnostic line + exit, so the Monitor fires a final
 *     event and the agent knows to restart the monitor (deafness is loud,
 *     never silent)
 */

import WebSocket from "ws";

const agent = process.argv[2];
if (!agent) {
  console.log("HUB_CLIENT_ERROR missing agent name — usage: hub-client.js <agent-name>");
  process.exit(2);
}

const url = process.env.APERTURE_HUB_URL ?? "ws://127.0.0.1:4517";
const token = process.env.APERTURE_HUB_TOKEN;

const ws = new WebSocket(url, { perMessageDeflate: false });

ws.on("open", () => {
  const hello: Record<string, unknown> = { type: "hello", role: "agent", agent };
  if (token) hello.token = token;
  ws.send(JSON.stringify(hello));
});

ws.on("message", (data) => {
  console.log(data.toString());
});

ws.on("close", (code, reason) => {
  console.log(`HUB_SOCKET_CLOSED code=${code} reason=${reason.toString()} — restart your inbox monitor`);
  process.exit(0);
});

ws.on("error", (err) => {
  console.log(`HUB_SOCKET_ERROR ${err.message} — hub unreachable? retry your inbox monitor shortly`);
  process.exit(1);
});
