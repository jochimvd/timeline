/** Period summaries, comparisons and per-day series over `focus_metrics` days.
 *  Pure functions of the server response and the current time, so the week and
 *  month views only render. Nothing here judges the work: changes are changes. */
import { duration, formatter } from "./format";
import type { CategoryGrouping } from "./categories";
import { hourSwitches, perHour, type SwitchKind } from "./metrics";
import type { DayFocusMetrics, FocusStretch, Range } from "./types";

/** A day that has not started yet, a past day without any observed evidence
 *  (missing evidence, not zero activity), or a day with evidence. */
export type DayState = "future" | "missing" | "recorded";
export const dayState = (day: DayFocusMetrics, now: number): DayState =>
  day.range.start_ms >= now ? "future" : day.evidence.observed_ms > 0 ? "recorded" : "missing";

export type PeriodSummary = {
  focus_ms: number; active_ms: number; idle_ms: number;
  switches: { application: number; category: number | null };
  long_ms: number; long_count: number;
  /** Earliest of the longest stretches, with its activity date. */
  longest: { stretch: FocusStretch; date: string } | null;
  observed_ms: number; gap_ms: number; unknown_ms: number;
  /** Days that have begun, and those of them without observed evidence. */
  elapsed_days: number; missing: string[];
};
export function summarize(days: DayFocusMetrics[], now: number): PeriodSummary {
  const summary: PeriodSummary = {
    focus_ms: 0, active_ms: 0, idle_ms: 0, switches: { application: 0, category: null }, long_ms: 0, long_count: 0,
    longest: null, observed_ms: 0, gap_ms: 0, unknown_ms: 0, elapsed_days: 0, missing: [],
  };
  for (const day of days) {
    const { metrics } = day;
    summary.focus_ms += day.totals.focus_ms;
    summary.active_ms += metrics.active_ms;
    summary.switches.application += metrics.application_switches;
    if (metrics.category_switches !== null) summary.switches.category = (summary.switches.category ?? 0) + metrics.category_switches;
    summary.long_ms += metrics.stretches.long.elapsed_ms;
    summary.long_count += metrics.stretches.long.count;
    if (metrics.longest_stretch && metrics.longest_stretch.elapsed_ms > (summary.longest?.stretch.elapsed_ms ?? 0))
      summary.longest = { stretch: metrics.longest_stretch, date: day.date };
    summary.observed_ms += day.evidence.observed_ms;
    summary.gap_ms += day.evidence.gap_ms;
    summary.unknown_ms += day.evidence.unknown_ms;
    const state = dayState(day, now);
    if (state !== "future") summary.elapsed_days++;
    if (state === "missing") summary.missing.push(day.date);
  }
  summary.idle_ms = summary.focus_ms - summary.active_ms;
  return summary;
}
export const switchRate = (summary: Pick<PeriodSummary, "switches" | "active_ms">, kind: SwitchKind) =>
  kind === "category" && summary.switches.category !== null
    ? perHour(summary.switches.category, summary.active_ms) : perHour(summary.switches.application, summary.active_ms);
export const longShare = (summary: Pick<PeriodSummary, "long_ms" | "active_ms">) =>
  summary.active_ms > 0 ? 100 * summary.long_ms / summary.active_ms : null;

/** A change against the comparison period, shown with an arrow in text colors. */
export type Delta = { text: string; direction: -1 | 0 | 1; label: string };
function delta(difference: number, text: string, label: string): Delta {
  return { text: difference === 0 ? "No change" : (difference > 0 ? "+" : "−") + text, direction: Math.sign(difference) as Delta["direction"], label };
}
export const durationDelta = (current: number, previous: number, label: string) => {
  // Compare displayed minutes, so "+0 min" never appears.
  const difference = Math.round(current / 60_000) - Math.round(previous / 60_000);
  return delta(difference, duration(Math.abs(difference) * 60_000), label);
};
export const rateDelta = (current: number | null, previous: number | null, label: string) =>
  current === null || previous === null ? undefined
    : delta(Math.round((current - previous) * 10), `${Math.abs(current - previous).toFixed(1)}/h`, label);

/** A stacked series: a category, an application, or the remaining applications. */
export type Subject = {
  key: string; name: string;
  /** Category name (null: uncategorized) or application, for its color; neither for "other". */
  category?: string | null; application?: string;
  /** Active focus per day, in day order. */
  days: number[]; total: number;
};
export const OTHER = "other";
const categoryKey = (category: string | null) => "category:" + (category ?? "");
const applicationKey = (application: string) => "application:" + application;

/** Active focus by subject over all days, keyed like `Subject.key`; categories
 *  roll up to the level `categories` shows. */
export function activeBySubject(days: DayFocusMetrics[], mode: SwitchKind, categories?: CategoryGrouping) {
  const totals = new Map<string, number>();
  for (const day of days) {
    const rows = mode === "category"
      ? day.categories.map(row => [categoryKey(categories ? categories.at(row.category) : row.category), row.active_ms] as const)
      : day.applications.map(row => [applicationKey(row.application), row.active_ms] as const);
    for (const [key, active] of rows) totals.set(key, (totals.get(key) ?? 0) + active);
  }
  return totals;
}

/** Stack series of active focus per day: the categories `categories` shows in
 *  tree order then uncategorized, or the `limit` applications with most active
 *  focus then the rest as one series. Only subjects with active focus in the period. */
export function subjects(days: DayFocusMetrics[], mode: SwitchKind, categories: CategoryGrouping, limit = 7): Subject[] {
  if (mode === "category") {
    return [...categories.order, null].map(category => {
      const per = days.map(day => sum(day.categories.filter(row => categories.at(row.category) === category).map(row => row.active_ms)));
      return { key: categoryKey(category), name: category ?? "Uncategorized", category, days: per, total: sum(per) };
    }).filter(subject => subject.total > 0);
  }
  const ranked = [...activeBySubject(days, "application")].filter(([, total]) => total > 0)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  // Folding a single application into "other" would hide its name for nothing.
  const shown = ranked.length > limit + 1 ? ranked.slice(0, limit) : ranked;
  const result: Subject[] = shown.map(([key]) => {
    const application = key.slice("application:".length);
    const per = days.map(day => day.applications.find(row => row.application === application)?.active_ms ?? 0);
    return { key, name: application, application, days: per, total: sum(per) };
  });
  if (shown.length < ranked.length) {
    const names = new Set(result.map(subject => subject.application));
    const per = days.map(day => sum(day.applications.filter(row => !names.has(row.application)).map(row => row.active_ms)));
    result.push({ key: OTHER, name: `${ranked.length - shown.length} other applications`, days: per, total: sum(per) });
  }
  return result;
}

/** One subject's active focus in both periods. `previous` is null without a comparison. */
export type SubjectChange = { subject: Subject; current: number; previous: number | null };
/** Subjects of the current period with their comparison totals; with categories,
 *  also categories that only had focus in the comparison period. */
export function compareSubjects(current: DayFocusMetrics[], previous: DayFocusMetrics[] | null,
  mode: SwitchKind, categories: CategoryGrouping, limit = 10): SubjectChange[] {
  const shown = subjects(current, mode, categories, limit);
  const before = previous && activeBySubject(previous, mode, categories);
  const rows: SubjectChange[] = shown.map(subject => ({ subject, current: subject.total, previous: null }));
  if (before) {
    const listed = new Set(shown.map(subject => subject.key));
    for (const row of rows) {
      row.previous = row.subject.key === OTHER
        ? sum([...before].filter(([key]) => !listed.has(key)).map(([, total]) => total))
        : before.get(row.subject.key) ?? 0;
    }
    if (mode === "category") {
      for (const category of [...categories.order, null]) {
        const key = categoryKey(category), total = before.get(key) ?? 0;
        if (total > 0 && !listed.has(key))
          rows.push({ subject: { key, name: category ?? "Uncategorized", category, days: current.map(() => 0), total: 0 }, current: 0, previous: total });
      }
      const order = [...categories.order, null].map(categoryKey);
      rows.sort((a, b) => order.indexOf(a.subject.key) - order.indexOf(b.subject.key));
    }
  }
  return rows;
}

/** Per-day focus-metric series; null where a day has no active focus. */
export function trendSeries(days: DayFocusMetrics[], kind: SwitchKind) {
  return {
    rate: days.map(day => switchRate({ switches: { application: day.metrics.application_switches,
      category: day.metrics.category_switches }, active_ms: day.metrics.active_ms }, kind)),
    longest: days.map(day => day.metrics.longest_stretch?.elapsed_ms ?? null),
    longShare: days.map(day => longShare({ long_ms: day.metrics.stretches.long.elapsed_ms, active_ms: day.metrics.active_ms })),
  };
}

/** Local clock hours in activity-day order, 04:00 first. */
export const HEAT_HOURS = Array.from({ length: 24 }, (_, index) => (index + 4) % 24);
export type HeatCell = {
  hour: number; range: Range; active_ms: number; switches: number; observed_ms: number;
  /** Not yet begun, without observed evidence, or with evidence. */
  state: DayState;
};
const HOUR = 3_600_000;
/** Hour-of-day cells of each day, in `HEAT_HOURS` order. A local hour missing on a
 *  daylight-saving day is null; a repeated one combines both. */
export function heatmap(days: DayFocusMetrics[], kind: SwitchKind, timezone: string, now: number) {
  const hourOf = formatter(timezone, "hour", { hour: "2-digit", hourCycle: "h23" });
  return days.map(day => {
    const cells: (HeatCell | null)[] = HEAT_HOURS.map(() => null);
    const hours = new Map(day.metrics.hours.map(hour => [hour.range.start_ms, hour]));
    const observed = day.evidence.intervals.filter(interval => interval.kind === "observed");
    for (let start = day.range.start_ms; start < day.range.end_ms; start += HOUR) {
      const range = { start_ms: start, end_ms: Math.min(start + HOUR, day.range.end_ms) };
      const hour = Number(hourOf.format(start)) % 24;
      const metrics = hours.get(start);
      const seen = sum(observed.map(interval => Math.max(0,
        Math.min(interval.range.end_ms, range.end_ms) - Math.max(interval.range.start_ms, range.start_ms))));
      const index = HEAT_HOURS.indexOf(hour), cell = cells[index];
      const active = metrics?.active_ms ?? 0, switches = metrics ? hourSwitches(metrics, kind) : 0;
      if (cell) {
        cell.range.end_ms = range.end_ms;
        cell.active_ms += active; cell.switches += switches; cell.observed_ms += seen;
        if (cell.observed_ms > 0) cell.state = "recorded";
      } else {
        cells[index] = { hour, range, active_ms: active, switches, observed_ms: seen,
          state: start >= now ? "future" : seen > 0 ? "recorded" : "missing" };
      }
    }
    return { date: day.date, cells };
  });
}

const sum = (values: number[]) => values.reduce((total, value) => total + value, 0);

/** A recording gap reason in words, such as "paused". */
export const gapLabel = (reason: string) => GAP_REASONS[reason] ?? reason.replaceAll("_", " ");
const GAP_REASONS: Record<string, string> = {
  pause: "paused", lock: "locked", disconnect: "desktop disconnected", restart: "recorder restarted",
  startup: "recorder started", suspend: "suspended", clock_discontinuity: "clock change",
  observation_stall: "recorder stalled", monotonic_reset: "clock reset",
};
