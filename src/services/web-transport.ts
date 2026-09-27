import type { CommandCall } from "./command-transport";
export const SESSION_KEY = "aperture.web.session.v1";
export const CANONICAL_ORIGIN = "http://127.0.0.1:4519";
const ended = () => ({ code: "E_WEB_SESSION_ENDED", message: "Session ended; reopen Aperture" });
const invalid = () => ({ code: "E_WEB_REQUEST", message: "Invalid browser command" });
const obj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const token = (v: unknown): v is string => typeof v === "string" && /^[A-Za-z0-9_-]{43}$/.test(v);
interface WebEnvironment {
  fetch: typeof fetch;
  storage: Pick<Storage, "getItem" | "setItem" | "removeItem">;
  onEnded: () => void;
}
function selector(value: unknown): string {
  if (typeof value !== "string" || !/^[a-zA-Z0-9_-]{1,128}$/.test(value)) throw invalid();
  return encodeURIComponent(value);
}
export function commandRoute(command: string, args: Record<string, unknown> = {}): { path: string; body?: Record<string, unknown> } {
  if (!obj(args)) throw invalid();
  const allowed = (keys: string[]) => { if (Object.keys(args).some(k => !keys.includes(k))) throw invalid(); };
  const read: Record<string, string> = { get_version: "/api/version", list_agents: "/api/agents", team_get_catalog: "/api/teams/catalog", team_list_presets: "/api/teams/presets", team_list: "/api/teams" };
  if (Object.prototype.hasOwnProperty.call(read, command)) {
    if (Object.keys(args).length) throw invalid();
    return { path: read[command] };
  }
  const agent: Record<string, string> = { start_agent: "start", stop_agent: "stop", restart_agent: "restart", update_agent_model: "model", clear_attention: "attention/clear" };
  if (Object.prototype.hasOwnProperty.call(agent, command)) {
    allowed(command === "update_agent_model" ? ["name", "model"] : ["name"]);
    return { path: `/api/agents/${selector(args.name)}/${agent[command]}`, body: args };
  }
  if (command === "tmux_create_session") { allowed(["sessionName"]); return { path: "/api/tmux/session", body: { session_name: args.sessionName } }; }
  if (command === "tmux_select_window") { allowed(["windowId"]); return { path: "/api/tmux/select-window", body: { window_id: args.windowId } }; }
  if (!obj(args.input) || Object.keys(args).some(k => k !== "input")) throw invalid();
  const body = args.input;
  if (command === "team_save_preset") return { path: "/api/teams/presets", body };
  if (command === "team_create") return { path: "/api/teams", body };
  const team = selector(body.team);
  switch (command) {
    case "team_cancel_pending": return { path: `/api/teams/${team}/cancel`, body };
    case "team_prepare_replacement": return { path: `/api/teams/${team}/replacement/prepare`, body };
    case "team_start_replacement": return { path: `/api/teams/${team}/replacement/start`, body };
    case "team_archive": return { path: `/api/teams/${team}/archive`, body };
    case "team_open_seat": return { path: `/api/teams/${team}/seats/${selector(body.seat)}/open`, body };
    case "team_bootstrap_seat": return { path: `/api/teams/${team}/seats/${selector(body.seat)}/bootstrap`, body };
    default: throw invalid();
  }
}
export function createWebTransport(env: WebEnvironment) {
  function clear(): void { env.storage.removeItem(SESSION_KEY); env.onEnded(); }
  async function request(path: string, body?: Record<string, unknown>, authenticated = true): Promise<unknown> {
    const headers: Record<string, string> = {};
    if (authenticated) {
      const session = env.storage.getItem(SESSION_KEY);
      if (!token(session)) { clear(); throw ended(); }
      headers.Authorization = `Bearer ${session}`;
    }
    if (body !== undefined) headers["Content-Type"] = "application/json";
    let response: Response;
    try {
      response = await env.fetch(path, { method: body === undefined ? "GET" : "POST", headers, body: body === undefined ? undefined : JSON.stringify(body), credentials: "omit", cache: "no-store", redirect: "error", referrerPolicy: "no-referrer" });
    } catch { throw { code: "E_WEB_OUTCOME_UNKNOWN", message: "Request outcome unknown; refresh state before another operation" }; }
    if (response.status === 401) { clear(); throw ended(); }
    let data: unknown;
    try { data = await response.json(); } catch { throw { code: "E_RESPONSE_INVALID", message: "Invalid response" }; }
    if (!response.ok) {
      if (obj(data) && typeof data.code === "string" && /^E_[A-Z0-9_]{1,80}$/.test(data.code) && typeof data.message === "string" && data.message.length <= 256) throw data;
      throw { code: "E_RESPONSE_INVALID", message: "Invalid error response" };
    }
    return data;
  }
  const call: CommandCall = async <T>(command: string, args?: Record<string, unknown>) => {
    const route = commandRoute(command, args);
    return await request(route.path, route.body) as T;
  };
  return {
    call,
    async exchange(value: string): Promise<void> {
      if (!token(value)) { clear(); throw ended(); }
      const result = await request("/session", { exchange: value }, false);
      if (!obj(result) || Object.keys(result).join() !== "session" || !token(result.session)) { clear(); throw invalid(); }
      env.storage.setItem(SESSION_KEY, result.session);
    },
    async resume(): Promise<void> { await call("get_version"); },
    async logout(): Promise<void> {
      const result = await request("/session/logout", {});
      if (!obj(result) || result.revoked !== true) throw invalid();
      clear();
    },
    async link(): Promise<string> {
      const result = await request("/session/link", {});
      if (!obj(result) || Object.keys(result).join() !== "exchange" || !token(result.exchange)) throw invalid();
      return `${CANONICAL_ORIGIN}/#t=${result.exchange}`;
    },
  };
}
/** Consume fragment synchronously, before any asynchronous work or main init. */
export function takeExchange(location: Pick<Location, "hash" | "pathname">, history: Pick<History, "replaceState">): string | null {
  const hash = location.hash;
  if (hash) history.replaceState(null, "", location.pathname);
  return hash.startsWith("#t=") ? hash.slice(3) : null;
}
