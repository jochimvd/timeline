import { readonly, writable } from "svelte/store";
import { query } from "./api";
import { addDays } from "./dates";
import { previousSpan, type Period } from "./routes";
import type { Session } from "./session";
import type { CategoryList, FocusMetricsByDay, Range } from "./types";

/** The previous period of equal length, cut where the current period's evidence
 *  ends when the current period is still in progress. */
export type Comparison = { first: string; last: string; range: Range; partial: boolean; data: FocusMetricsByDay };
export type PeriodState = {
  timezone: string;
  period: Period | null;
  /** Per-day focus of `period`, retained while a replacement loads. */
  current: FocusMetricsByDay | null;
  /** Loaded after `current`; null until then or without one. */
  comparison: Comparison | null;
  categories: CategoryList["categories"];
  /** The time the displayed period was read, separating past from future days. */
  now: number;
  loading: boolean; comparing: boolean; message: string;
};

/** Week and month data: one `focus_metrics` scan for the period, then one for its
 *  comparison. Newer periods abort and supersede older requests; views only render. */
export function createPeriodModel() {
  let state: PeriodState = {
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone, period: null, current: null, comparison: null,
    categories: [], now: Date.now(), loading: false, comparing: false, message: "",
  };
  const store = writable(state);
  const patch = (values: Partial<PeriodState>) => { state = { ...state, ...values }; store.set(state); };
  let generation = 0, abort = new AbortController(), active = false, disposed = false, started = false;
  let refreshTimer: ReturnType<typeof setInterval> | undefined;
  const resolved = new Map<string, Range>();

  async function dayRange(date: string, signal: AbortSignal) {
    let range = resolved.get(date);
    if (!range) {
      range = (await query<{ range: Range }>("resolve_range", { date }, signal)).range;
      resolved.set(date, range);
      if (resolved.size > 128) resolved.delete(resolved.keys().next().value!);
    }
    return range;
  }
  /** Reads the previous span, or for a period in progress only up to the same
   *  activity day and time of day, so weekdays and working hours line up. */
  async function compare(period: Period, current: FocusMetricsByDay, now: number, signal: AbortSignal): Promise<Comparison> {
    const span = previousSpan(period);
    const today = current.days.findIndex(day => day.range.start_ms <= now && now < day.range.end_ms);
    if (today < 0) {
      const data = await query<FocusMetricsByDay>("focus_metrics", { dates: span }, signal);
      return { ...span, partial: false, range: data.resolved.range, data };
    }
    const start = await dayRange(span.first, signal);
    const same = await dayRange(addDays(span.first, today), signal);
    const end = Math.min(same.end_ms, same.start_ms + Math.max(1, now - current.days[today].range.start_ms));
    const range = { start_ms: start.start_ms, end_ms: end };
    const data = await query<FocusMetricsByDay>("focus_metrics", { range }, signal);
    return { first: span.first, last: addDays(span.first, today), partial: true, range, data };
  }
  async function load(period: Period, background = false) {
    const current = ++generation;
    abort.abort();
    abort = new AbortController();
    const signal = abort.signal;
    patch({ period, loading: !background, ...(background ? {} : { message: "" }) });
    try {
      const [data, categories] = await Promise.all([
        query<FocusMetricsByDay>("focus_metrics", { dates: { first: period.first, last: period.last } }, signal),
        query<CategoryList>("categories", {}, signal),
      ]);
      if (current !== generation) return;
      const now = Date.now();
      const sameComparison = state.comparison && state.period?.first === period.first && state.current?.resolved.range.start_ms === data.resolved.range.start_ms;
      patch({ current: data, categories: categories.categories, timezone: data.resolved.timezone, now, loading: false,
        comparing: true, ...(sameComparison && background ? {} : { comparison: null }) });
      if (data.resolved.range.start_ms >= now) { patch({ comparing: false, comparison: null }); return; }
      const comparison = await compare(period, data, now, signal);
      if (current !== generation) return;
      patch({ comparison, comparing: false });
    } catch (error) {
      if (current !== generation || signal.aborted) return;
      patch({ loading: false, comparing: false, message: error instanceof Error ? error.message : String(error) });
    }
  }
  /** Reloads an in-progress period while shown, keeping the displayed one meanwhile. */
  function refresh() {
    if (disposed || !active || document.hidden || !state.period || state.loading || !state.current) return;
    if (state.current.resolved.range.end_ms <= Date.now()) return;
    void load(state.period, true);
  }
  function becameVisible() { if (!document.hidden) refresh(); }
  function start(session: Session) {
    if (started) return;
    started = true;
    patch({ timezone: session.timezone });
    refreshTimer = setInterval(refresh, 60_000);
    document.addEventListener("visibilitychange", becameVisible);
    window.addEventListener("focus", becameVisible);
  }
  function setActive(value: boolean) { active = value; }
  function dispose() {
    disposed = true;
    generation++;
    abort.abort();
    clearInterval(refreshTimer);
    if (!started) return;
    document.removeEventListener("visibilitychange", becameVisible);
    window.removeEventListener("focus", becameVisible);
  }
  return { state: readonly(store), start, load, setActive, dispose };
}
export type PeriodModel = ReturnType<typeof createPeriodModel>;
