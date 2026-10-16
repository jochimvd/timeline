import { readonly, writable } from "svelte/store";
import { request } from "./api";
import { SignedOut } from "./connection";

export type RecordingState = { label: string; paused: boolean; busy: boolean; error: string };
type Status = {
  recording: { manual_paused: boolean; locked: boolean; connected: boolean; screenshot_pressure?: boolean };
  desktop_error?: string; screenshot_error?: string;
};

/** Recorder status for the page header, polled every five seconds, and the
 *  durable pause/resume control. Independent of which view is shown. */
export function createRecording() {
  const store = writable<RecordingState>({ label: "Connecting…", paused: false, busy: true, error: "" });
  const lifetime = new AbortController();
  let timer: ReturnType<typeof setInterval> | undefined;
  let state: RecordingState;
  store.subscribe(value => state = value);
  async function status() {
    try {
      const s = await request<Status>("/api/status", undefined, lifetime.signal);
      let label = s.recording.manual_paused ? "Recording paused" : s.recording.locked
        ? "Screen locked" : !s.recording.connected ? "Desktop disconnected" : "Recording";
      if (s.desktop_error || s.screenshot_error || s.recording.screenshot_pressure) label += " · recording degraded";
      store.set({ label, paused: s.recording.manual_paused, busy: false, error: "" });
    } catch {
      if (!lifetime.signal.aborted) store.update(value => ({ ...value, label: "Recording status unavailable", busy: true }));
    }
  }
  async function toggle() {
    store.update(value => ({ ...value, busy: true }));
    let failure = "";
    try { await request("/api/recording/" + (state.paused ? "resume" : "pause"), {}, lifetime.signal); }
    catch (error) { if (!(error instanceof SignedOut)) failure = error instanceof Error ? error.message : String(error); }
    await status();
    if (failure && !lifetime.signal.aborted) store.update(value => ({ ...value, error: failure }));
  }
  async function start() {
    await status();
    if (!lifetime.signal.aborted) timer = setInterval(() => void status(), 5000);
  }
  function dispose() { lifetime.abort(); clearInterval(timer); }
  return { state: readonly(store), start, toggle, dispose };
}
