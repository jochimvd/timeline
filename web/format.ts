import type { Coverage } from "./types";
export const imageTransform = (transform: number) =>
  "rotate(" + ([0, -90, -180, -270, 0, -90, -180, -270][transform] || 0) + "deg)" + (transform >= 4 ? " scaleX(-1)" : "");
/** Focus duration for totals: "<1 min", "42 min" or "1h 5m". */
export function duration(ms: number) {
  if (ms > 0 && ms < 60_000) return "<1 min";
  const minutes = Math.round(ms / 60_000);
  return minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)}h${minutes % 60 ? ` ${minutes % 60}m` : ""}`;
}
/** Like `duration`, but in seconds below a minute. */
export function elapsed(ms: number) {
  return ms < 60_000 ? `${Math.round(ms / 1000)} s` : duration(ms);
}
// Intl formatter construction is expensive; labels share one per timezone and style.
const formatters = new Map<string, Intl.DateTimeFormat>();
export function formatter(timezone: string, style: string, options: Intl.DateTimeFormatOptions) {
  const key = style + " " + timezone;
  let cached = formatters.get(key);
  if (!cached) formatters.set(key, cached = new Intl.DateTimeFormat("en-GB", { timeZone: timezone, ...options }));
  return cached;
}
export const time = (ms: number, timezone: string) =>
  formatter(timezone, "time", { hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(ms);
/** Hours and minutes, for ranges such as stretches and hourly buckets. */
export const clock = (ms: number, timezone: string) =>
  formatter(timezone, "clock", { hour: "2-digit", minute: "2-digit" }).format(ms);
export const clockRange = (range: { start_ms: number; end_ms: number }, timezone: string) =>
  `${clock(range.start_ms, timezone)}–${clock(range.end_ms, timezone)}`;
export const noScreenshot = (expired: boolean) => expired
  ? "Screenshots from this time have expired; activity is kept longer."
  : "No screenshot evidence at this time.";
export const captureTime = (ms: number, timezone: string) =>
  formatter(timezone, "capture", { dateStyle: "medium", timeStyle: "medium" }).format(ms);
/** Stable application hue; light enough for dark label ink in both themes. */
export const color = (name: string) => `hsl(${Array.from(name).reduce((n, c) => (n * 31 + c.charCodeAt(0)) % 360, 0)} 44% 61%)`;

/** A mark's fill with label ink that stays readable on it. */
export type Swatch = { fill: string; ink: string };
export const applicationSwatch = (name: string): Swatch => ({ fill: color(name), ink: "var(--ink-on-fill)" });
export const UNCATEGORIZED = "Uncategorized";
/** Category fill: an explicit color, else its theme palette slot, else neutral.
 *  A nonzero `shade` tells apart subcategories sharing their parent's color:
 *  odd shades lighten and even ones darken, more with each pair. */
export function categorySwatch(category?: { color?: string | null; palette?: number | null }, shade = 0): Swatch {
  const base: Swatch = category?.color ? { fill: category.color, ink: readableInk(category.color) }
    : category?.palette ? { fill: `var(--series-${category.palette})`, ink: `var(--series-${category.palette}-ink)` }
    : { fill: "var(--uncategorized)", ink: "var(--uncategorized-ink)" };
  if (!shade) return base;
  const amount = Math.min(70, 30 * Math.ceil(shade / 2));
  return shade % 2
    ? { fill: `color-mix(in oklab, ${base.fill}, white ${amount}%)`, ink: "#0b0f14" }
    : { fill: `color-mix(in oklab, ${base.fill}, black ${amount}%)`, ink: "#ffffff" };
}
/** Dark or light ink, whichever contrasts more with a `#rrggbb` fill. */
function readableInk(hex: string) {
  const channel = (offset: number) => {
    const value = parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  const luminance = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
  // Contrast with near-black (L 0.004) beats white once luminance exceeds ~0.18.
  return (luminance + 0.05) / 0.054 >= 1.05 / (luminance + 0.05) ? "#0b0f14" : "#ffffff";
}
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
