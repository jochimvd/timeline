import { readonly, writable } from "svelte/store";
import { query } from "./api";
import type { DayFocusMetrics, FocusMetricsByDay, FocusSession, SessionList } from "./types";

/** Whole-day figures for the day view, independent of the timeline's zoom:
 *  the day's focus metrics and its sessions, read together per date. */
export type DayDetails = {
  date: string;
  day: DayFocusMetrics | null;
  sessions: FocusSession[] | null;
  loading: boolean;
  error: string;
};

export function createDayDetails() {
  let state: DayDetails = { date: "", day: null, sessions: null, loading: false, error: "" };
  const store = writable(state);
  const patch = (values: Partial<DayDetails>) => { state = { ...state, ...values }; store.set(state); };
  let abort = new AbortController(), generation = 0, disposed = false;
  let timer: ReturnType<typeof setInterval> | undefined;
  /** When the shown day was last read, on the monotonic clock. */
  let readAt = -Infinity;

  async function load(date: string, background = false) {
    if (!date || disposed) return;
    const current = ++generation;
    abort.abort();
    abort = new AbortController();
    const signal = abort.signal;
    patch(background ? { loading: true } : { date, loading: true, error: "", ...(state.date === date ? {} : { day: null, sessions: null }) });
    readAt = performance.now();
    try {
      // Sequential, so the day view never holds more than one extra admission slot.
      const metrics = await query<FocusMetricsByDay>("focus_metrics", { date }, signal);
      const sessions = await query<SessionList>("sessions", { date }, signal);
      if (current !== generation) return;
      patch({ date, day: metrics.days[0] ?? null, sessions: sessions.sessions, loading: false, error: "" });
    } catch (error) {
      if (current !== generation || signal.aborted) return;
      patch({ loading: false, error: error instanceof Error ? error.message : String(error) });
    }
  }
  /** Reloads a day in progress about once a minute while the page is visible. */
  function start() {
    timer = setInterval(() => {
      const day = state.day;
      if (!document.hidden && day && day.range.start_ms <= Date.now() && Date.now() < day.range.end_ms && !state.loading)
        void load(state.date, true);
    }, 60_000);
  }
  let trailing: ReturnType<typeof setTimeout> | undefined;
  /** Live evidence changed the shown day: reread it, at most every 15 seconds. */
  function changed(date: string) {
    if (date !== state.date || trailing !== undefined) return;
    trailing = setTimeout(() => {
      trailing = undefined;
      if (date === state.date && !disposed) void load(date, true);
    }, Math.max(0, readAt + 15_000 - performance.now()));
  }
  function dispose() { disposed = true; generation++; abort.abort(); clearInterval(timer); clearTimeout(trailing); }
  return { state: readonly(store), load, start, changed, dispose };
}
export type DayDetailsModel = ReturnType<typeof createDayDetails>;
