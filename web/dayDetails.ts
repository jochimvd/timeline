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

/** The timeline model reads a day's details with its timeline and shows both
 *  at once; these only refresh the shown day on their own. */
export function createDayDetails() {
  let state: DayDetails = { date: "", day: null, sessions: null, loading: false, error: "" };
  const store = writable(state);
  const patch = (values: Partial<DayDetails>) => { state = { ...state, ...values }; store.set(state); };
  let abort = new AbortController(), generation = 0, disposed = false;
  let timer: ReturnType<typeof setInterval> | undefined;
  /** When the shown day was last read, on the monotonic clock. */
  let readAt = -Infinity;

  /** Reads `date`'s details without showing them; a failure is part of the
   *  result, so the day still shows. Null once `signal` aborts. */
  async function read(date: string, signal: AbortSignal): Promise<DayDetails | null> {
    readAt = performance.now();
    try {
      // Sequential, so the day view never holds more than one extra admission slot.
      const metrics = await query<FocusMetricsByDay>("focus_metrics", { date }, signal);
      const sessions = await query<SessionList>("sessions", { date }, signal);
      return { date, day: metrics.days[0] ?? null, sessions: sessions.sessions, loading: false, error: "" };
    } catch (error) {
      if (signal.aborted) return null;
      return { date, day: null, sessions: null, loading: false, error: error instanceof Error ? error.message : String(error) };
    }
  }
  /** Shows details read for a newly shown day, superseding any reload. */
  function show(details: DayDetails) {
    generation++;
    abort.abort();
    patch(details);
  }
  /** Rereads the shown day, keeping its details meanwhile. */
  async function reload() {
    if (!state.date || disposed) return;
    const current = ++generation;
    abort.abort();
    abort = new AbortController();
    patch({ loading: true });
    const details = await read(state.date, abort.signal);
    if (current !== generation || !details) return;
    patch(details.error ? { loading: false, error: details.error } : details);
  }
  /** Reloads a day in progress about once a minute while `visible`. */
  function start(visible: () => boolean) {
    timer = setInterval(() => {
      const day = state.day;
      if (visible() && day && day.range.start_ms <= Date.now() && Date.now() < day.range.end_ms && !state.loading)
        void reload();
    }, 60_000);
  }
  let trailing: ReturnType<typeof setTimeout> | undefined;
  /** Live evidence changed the shown day: reread it, at most every 15 seconds. */
  function changed(date: string) {
    if (date !== state.date || trailing !== undefined) return;
    trailing = setTimeout(() => {
      trailing = undefined;
      if (date === state.date && !disposed) void reload();
    }, Math.max(0, readAt + 15_000 - performance.now()));
  }
  function dispose() { disposed = true; generation++; abort.abort(); clearInterval(timer); clearTimeout(trailing); }
  return { state: readonly(store), read, show, start, changed, dispose };
}
