import WebSocket from "ws";

/**
 * Best-effort push notification to the comms-layer v2 WS hub (ws-hub.ts).
 *
 * Called from the send-queue drain path AFTER a message has successfully
 * landed in BEADS. The hub forwards a {type:"message"} event to the recipient
 * if their Monitor socket is connected; if the hub is down or the recipient
 * offline, this is a silent no-op — the hub's unread replay covers it.
 *
 * MUST NEVER throw or hang: resolves within HUB_NOTIFY_TIMEOUT_MS regardless
 * of outcome. send_message durability lives entirely in the BEADS queue.
 */

const HUB_PORT = Number(process.env.APERTURE_WS_PORT ?? 4517);
const HUB_NOTIFY_TIMEOUT_MS = 1500;

export interface HubNotification {
  to: string;
  id: string;
  from: string;
  preview: string;
}

export function notifyHub(
  note: HubNotification,
  log: (msg: string) => void = (m) => console.error(m),
): Promise<void> {
  return new Promise((resolve) => {
    let settled = false;
    let ws: WebSocket | null = null;

    const finish = (reason?: string) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (reason) log(`[hub-notify] ${reason} (id=${note.id}, to=${note.to}) — replay will cover delivery`);
      try {
        ws?.close();
      } catch {
        // already closed/terminated
      }
      resolve();
    };

    const timer = setTimeout(() => {
      try {
        ws?.terminate();
      } catch {
        // ignore
      }
      finish("timed out awaiting hub ack");
    }, HUB_NOTIFY_TIMEOUT_MS);
    timer.unref?.();

    try {
      ws = new WebSocket(`ws://127.0.0.1:${HUB_PORT}`);
    } catch (e: unknown) {
      finish(`hub connect failed: ${e instanceof Error ? e.message : String(e)}`);
      return;
    }

    ws.on("open", () => {
      ws!.send(JSON.stringify({ type: "hello", role: "producer" }));
      ws!.send(JSON.stringify({ type: "notify", ...note }));
    });

    ws.on("message", (data) => {
      try {
        const msg = JSON.parse(data.toString());
        if (msg?.type === "ok") finish();
      } catch {
        // ignore non-JSON frames; timeout is the backstop
      }
    });

    ws.on("error", (err) => {
      finish(`hub unavailable: ${err.message}`);
    });

    ws.on("close", () => {
      finish();
    });
  });
}
