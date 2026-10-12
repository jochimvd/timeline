/** Calendar arithmetic on activity dates (YYYY-MM-DD). An activity day runs
 *  from 04:00 local time until 04:00 the next date; exact boundaries come
 *  from the server, which accounts for daylight saving. */

// Intl formatter construction is expensive; activity dates share one per timezone.
const activityFormatters = new Map<string, Intl.DateTimeFormat>();

/** The activity date containing `ms` in `timezone`. */
export function activityDate(ms: number, timezone: string) {
  let formatter = activityFormatters.get(timezone);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat("en-CA", { timeZone: timezone,
      year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", hourCycle: "h23" });
    activityFormatters.set(timezone, formatter);
  }
  const parts = formatter.formatToParts(ms);
  const part = (type: string) => parts.find(part => part.type === type)!.value;
  const date = `${part("year")}-${part("month")}-${part("day")}`;
  return Number(part("hour")) < 4 ? addDays(date, -1) : date;
}

const utc = (date: string) => new Date(date + "T12:00:00Z");
export const isDate = (value: string | null | undefined): value is string =>
  !!value && /^\d{4}-\d{2}-\d{2}$/.test(value) && !Number.isNaN(utc(value).getTime())
  && utc(value).toISOString().startsWith(value);
export function addDays(date: string, days: number) {
  const value = utc(date);
  value.setUTCDate(value.getUTCDate() + days);
  return value.toISOString().slice(0, 10);
}
/** Whole days from `a` to `b`. */
export const daysBetween = (a: string, b: string) => Math.round((utc(b).getTime() - utc(a).getTime()) / 86_400_000);
/** 0 for Monday through 6 for Sunday. */
export const weekday = (date: string) => (utc(date).getUTCDay() + 6) % 7;
/** Weeks start on Monday. */
export const weekStart = (date: string) => addDays(date, -weekday(date));
export const monthStart = (date: string) => date.slice(0, 8) + "01";
export function addMonths(date: string, months: number) {
  const value = utc(monthStart(date));
  value.setUTCMonth(value.getUTCMonth() + months);
  return value.toISOString().slice(0, 10);
}
export const daysInMonth = (date: string) => daysBetween(monthStart(date), addMonths(date, 1));

// Calendar labels are formatted in UTC: dates carry no time of day.
const labels = new Map<string, Intl.DateTimeFormat>();
export function dateLabel(date: string, options: Intl.DateTimeFormatOptions) {
  const key = JSON.stringify(options);
  let formatter = labels.get(key);
  if (!formatter) labels.set(key, formatter = new Intl.DateTimeFormat("en-GB", { timeZone: "UTC", ...options }));
  return formatter.format(utc(date));
}
/** "Mon 5 Oct". */
export const shortDate = (date: string) => dateLabel(date, { weekday: "short", day: "numeric", month: "short" });
/** "5–11 Oct 2026", "28 Sep – 4 Oct 2026" or "28 Dec 2026 – 3 Jan 2027". */
export function dateSpan(first: string, last: string) {
  if (first === last) return dateLabel(first, { day: "numeric", month: "short", year: "numeric" });
  const sameYear = first.slice(0, 4) === last.slice(0, 4);
  const end = dateLabel(last, { day: "numeric", month: "short", year: "numeric" });
  if (sameYear && first.slice(0, 7) === last.slice(0, 7)) return `${Number(first.slice(8))}–${end}`;
  return `${dateLabel(first, sameYear ? { day: "numeric", month: "short" } : { day: "numeric", month: "short", year: "numeric" })} – ${end}`;
}

/** Opens a date input's picker from a click anywhere on it, not only its own icon. */
export function openPicker(event: MouseEvent & { currentTarget: HTMLInputElement }) {
  try { event.currentTarget.showPicker(); } catch { /* Already open, or unsupported: the input still works. */ }
}
