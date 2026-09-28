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
  const onIncompatible = () => { status.textContent = "UI/API incompatible; reload Aperture"; link.disabled = true; };
  try {
    const web = createWebTransport({ fetch: window.fetch.bind(window), storage: window.sessionStorage, onEnded, onIncompatible });
    setCommandTransport(web.call);
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
    if (exchange !== null) await web.exchange(exchange);
    await web.resume(); // Includes schema gate before main init/polling/mutations.
    status.textContent = "Local operator session";
    return true;
  } catch (error) {
    if ((error as { code?: string })?.code === "E_WEB_API_INCOMPATIBLE") onIncompatible();
    else onEnded();
    return false;
  }
}
