import { addDays, addMonths, dateLabel, dateSpan, daysBetween, daysInMonth, isDate, monthStart, weekStart } from "./dates";

/** Week and month views summarize consecutive activity days. */
export type PeriodKind = "week" | "month";
/** What the URL selects: a day (the newest capture's day without a date), or a period. */
export type Route = { view: "day"; date?: string } | { view: PeriodKind; date: string };
/** Consecutive activity dates, both inclusive. */
export type Period = { kind: PeriodKind; first: string; last: string; length: number };

/** `?date=2026-10-07` (day), `?view=week&date=2026-10-05` or `?view=month&date=2026-10-01`. */
export function parseRoute(search: string): Route {
  const parameters = new URLSearchParams(search);
  const date = parameters.get("date"), view = parameters.get("view");
  if ((view === "week" || view === "month") && isDate(date)) return { view, date: period(view, date).first };
  return isDate(date) ? { view: "day", date } : { view: "day" };
}
export function routeSearch(route: Route) {
  if (route.view === "day") return route.date ? `?date=${route.date}` : "/";
  return `?view=${route.view}&date=${route.date}`;
}
export const sameRoute = (a: Route, b: Route) => routeSearch(a) === routeSearch(b);

/** The week (starting Monday) or calendar month of activity days containing `date`. */
export function period(kind: PeriodKind, date: string): Period {
  const first = kind === "week" ? weekStart(date) : monthStart(date);
  const length = kind === "week" ? 7 : daysInMonth(date);
  return { kind, first, last: addDays(first, length - 1), length };
}
export const shiftPeriod = (current: Period, delta: number) => period(current.kind,
  current.kind === "week" ? addDays(current.first, 7 * delta) : addMonths(current.first, delta));
/** The comparison period: the same number of activity days immediately before. For
 *  a week that is the previous week; for a month, as many days as it has. */
export function previousSpan(current: Period) {
  return { first: addDays(current.first, -current.length), last: addDays(current.first, -1) };
}
export const periodDates = (current: Period) =>
  Array.from({ length: current.length }, (_, index) => addDays(current.first, index));
export const containsDate = (current: Period, date: string) =>
  daysBetween(current.first, date) >= 0 && daysBetween(date, current.last) >= 0;

export const periodTitle = (current: Period) => current.kind === "week"
  ? `Week of ${dateSpan(current.first, current.last)}` : dateLabel(current.first, { month: "long", year: "numeric" });
/** Names the comparison period in deltas: "last week" or "previous 31 days". */
export const comparisonName = (current: Period) => current.kind === "week" ? "last week" : `previous ${current.length} days`;
/** Where an in-progress period's comparison stops: "the same point last week". */
export const samePoint = (current: Period) => current.kind === "week" ? "the same point last week" : `the same point ${current.length} days earlier`;
