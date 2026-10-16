import { request } from "./api";
import { lose, SignedOut } from "./connection";

/** What every view needs from the server before loading evidence. */
export type Session = { timezone: string; settingsPath: string | null };

/** Exchanges the launch credential in the URL fragment for a session cookie,
 *  keeping the rest of the URL (such as a linked view), then reads the config. */
export async function startSession(signal: AbortSignal): Promise<Session> {
  const token = location.hash.slice(1);
  let spent = false;
  if (token) {
    history.replaceState(history.state, "", location.pathname + location.search);
    // A spent link, such as one reopened from history, still opens the page while this browser's session lasts.
    try { await request("/bootstrap", { token }, signal); }
    catch (error) { if (!(error instanceof SignedOut && error.reason === "link_used")) throw error; spent = true; }
  }
  try {
    const config = await request<{ timezone: string; settings: string | null }>("/api/config", undefined, signal);
    return { timezone: config.timezone, settingsPath: config.settings };
  } catch (error) {
    if (spent && error instanceof SignedOut && error.reason === "expired") lose("link_used");
    throw error;
  }
}
