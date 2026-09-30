import type { Coverage } from "./types";
export const imageTransform = (transform: number) =>
  "rotate(" + ([0, -90, -180, -270, 0, -90, -180, -270][transform] || 0) + "deg)" + (transform >= 4 ? " scaleX(-1)" : "");
export const duration = (ms: number) => `${Math.round(ms / 60000)} min`;
// Intl formatter construction is expensive; timeline labels share one per timezone.
const timeFormatters = new Map<string, Intl.DateTimeFormat>();
const captureFormatters = new Map<string, Intl.DateTimeFormat>();
export function time(ms: number, timezone: string) {
  let formatter = timeFormatters.get(timezone);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat("en-GB", { timeZone: timezone, hour: "2-digit", minute: "2-digit", second: "2-digit" });
    timeFormatters.set(timezone, formatter);
  }
  return formatter.format(ms);
}
export function captureTime(ms: number, timezone: string) {
  let formatter = captureFormatters.get(timezone);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat("en-GB", { timeZone: timezone, dateStyle: "medium", timeStyle: "medium" });
    captureFormatters.set(timezone, formatter);
  }
  return formatter.format(ms);
}
export const color = (name: string) => `hsl(${Array.from(name).reduce((n, c) => (n * 31 + c.charCodeAt(0)) % 360, 0)} 44% 61%)`;
export function coverageBands(input: Coverage[]): Coverage[] {
  const bands: Coverage[] = [];
  for (const row of input) {
    if (!row.range) continue;
    const previous = bands.at(-1);
    if (previous?.range?.end_ms === row.range.start_ms &&
        previous.kind === row.kind && previous.mixed === row.mixed && previous.has_gap === row.has_gap
        && previous.idle === row.idle && previous.segment === row.segment &&
        previous.recording_boundary === row.recording_boundary &&
        previous.monitor === row.monitor && previous.unavailable === row.unavailable &&
        previous.focus?.kind === row.focus?.kind &&
        previous.focus?.idle === row.focus?.idle &&
        previous.focus?.reason === row.focus?.reason) {
      previous.range.end_ms = row.range.end_ms;
    } else {
      bands.push({ ...row, range: { ...row.range } });
    }
  }
  return bands;
}
