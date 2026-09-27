import { setCommandTransport } from "./command-transport";
import { CANONICAL_ORIGIN, createWebTransport, takeExchange } from "./web-transport";

export async function initializeBrowserSession(): Promise<boolean> {
  if ("__TAURI_INTERNALS__" in window) return true;
  // Remove the exchange fragment before fetching, rendering, or awaiting anything.
  const exchange = takeExchange(window.location, window.history);
  const panel = document.createElement("section");
  panel.className = "web-session";
  const status = document.createElement("span");
  status.setAttribute("role", "status");
  const logout = document.createElement("button"); logout.textContent = "Log out";
  const link = document.createElement("button"); link.textContent = "Open another window";
  panel.append(status, link, logout);
  document.getElementById("navbar")!.append(panel);
  const onEnded = () => { status.textContent = "Session ended; reopen Aperture"; link.disabled = logout.disabled = true; };
  if (window.location.origin !== CANONICAL_ORIGIN) { onEnded(); return false; }
  try {
    const web = createWebTransport({ fetch: window.fetch.bind(window), storage: window.sessionStorage, onEnded });
    setCommandTransport(web.call);
    if (exchange !== null) await web.exchange(exchange);
    else await web.resume();
    status.textContent = "Local operator session";
    logout.onclick = async () => {
      logout.disabled = true;
      try { await web.logout(); } catch { status.textContent = "Logout unconfirmed; retry or close this window"; logout.disabled = false; }
    };
    link.onclick = async () => {
      link.disabled = true;
      try { window.open(await web.link(), "_blank", "noopener,noreferrer"); }
      catch { status.textContent = "New window unavailable; reopen Aperture"; }
      finally { link.disabled = false; }
    };
    return true;
  } catch { onEnded(); return false; }
}
