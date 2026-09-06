import WebSocket from "ws";
import { readFileSync } from "node:fs";

/**
 * Best-effort push notification to the comms-layer v2 WS hub (ws-hub.ts).
 *
 * Called from the send-queue drain path AFTER a message has successfully
 * landed in BEADS. The hub forwards a {type:"message"} event to the recipient
 * if their Monitor socket is connected; if the hub is down or the recipient
 * offline, this is a no-op — the hub's unread replay covers it.
 *
 * Resolves with what actually happened (aperture-oeb6q — honest acks):
 *   "forwarded" — hub pushed to the recipient's Monitor socket
 *   "codex"     — hub injected a turn into the recipient's Codex bridge
 *   "offline"   — hub acked but nobody was there to push to (replay covers it)
 *   "unacked"   — no ack at all: hub down/unreachable, credential missing,
 *                 or timed out (replay covers it too)
 *
 * MUST NEVER throw or hang: resolves within HUB_NOTIFY_TIMEOUT_MS regardless
 * of outcome. send_message durability lives entirely in the BEADS queue.
 */

const HUB_PORT = Number(process.env.APERTURE_WS_PORT ?? 4517);
const AGENT_NAME = process.env.AGENT_NAME ?? "";
const TOKEN_FILE = process.env.APERTURE_HUB_TOKEN_FILE ?? "";
const HUB_NOTIFY_TIMEOUT_MS = 1500;

export interface HubNotification {
  to: string;
  id: string;
  from: string;
  preview: string;
}

/** Hub-reported outcomes (on the ok ack) plus the local "no ack" case. */
export type HubNotifyOutcome = "forwarded" | "codex" | "offline" | "unacked";

const ACKED_OUTCOMES: ReadonlySet<string> = new Set(["forwarded", "codex", "offline"]);

export function notifyHub(
  note: HubNotification,
  log: (msg: string) => void = (m) => console.error(m),
): Promise<HubNotifyOutcome> {
  return new Promise((resolve) => {
    let settled = false;
    let ws: WebSocket | null = null;

    const finish = (outcome: HubNotifyOutcome, reason?: string) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (reason) log(`[hub-notify] ${reason} (id=${note.id}, to=${note.to}) — replay will cover delivery`);
      try {
        ws?.close();
      } catch {
        // already closed/terminated
      }
      resolve(outcome);
    };

    const timer = setTimeout(() => {
      try {
        ws?.terminate();
      } catch {
        // ignore
      }
      finish("unacked", "timed out awaiting hub ack");
    }, HUB_NOTIFY_TIMEOUT_MS);
    timer.unref?.();

    try {
      ws = new WebSocket(`ws://127.0.0.1:${HUB_PORT}`);
    } catch (e: unknown) {
      finish("unacked", `hub connect failed: ${e instanceof Error ? e.message : String(e)}`);
      return;
    }

    ws.on("open", () => {
      let token: string;
      try {
        token = readFileSync(TOKEN_FILE, "utf8");
      } catch {
        finish("unacked", "hub credential unavailable");
        return;
      }
      ws!.send(JSON.stringify({ type: "hello", role: "producer", agent: AGENT_NAME, token }));
      ws!.send(JSON.stringify({ type: "notify", ...note }));
    });

    ws.on("message", (data) => {
      try {
        const msg = JSON.parse(data.toString());
        if (msg?.type === "ok") {
          // A pre-oeb6q hub acks without `outcome`. It DID ack, so "unacked"
          // would be wrong; read it conservatively as "offline" (replay covers).
          const outcome = ACKED_OUTCOMES.has(msg.outcome) ? (msg.outcome as HubNotifyOutcome) : "offline";
          finish(outcome);
        }
      } catch {
        // ignore non-JSON frames; timeout is the backstop
      }
    });

    ws.on("error", (err) => {
      finish("unacked", `hub unavailable: ${err.message}`);
    });

    ws.on("close", () => {
      // Closed before an ok arrived (hub rejected the hello, or shut down
      // mid-flight): no ack.
      finish("unacked", "hub closed before ack");
    });
  });
}
