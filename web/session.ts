import { request } from "./api";

/** What every view needs from the server before loading evidence. */
export type Session = { timezone: string; settingsPath: string | null };

/** Exchanges the launch credential in the URL fragment for a session cookie,
 *  keeping the rest of the URL (such as a linked view), then reads the config. */
export async function startSession(signal: AbortSignal): Promise<Session> {
  const token = location.hash.slice(1);
  if (token) {
    history.replaceState(history.state, "", location.pathname + location.search);
    await request("/bootstrap", { token }, signal);
  }
  const config = await request<{ timezone: string; settings: string | null }>("/api/config", undefined, signal);
  return { timezone: config.timezone, settingsPath: config.settings };
}
