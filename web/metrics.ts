import type { FocusMetrics, HourlyFocus } from "./types";

/** Which switches a view shows: between applications or between categories. */
export type SwitchKind = "application" | "category";

/** Lower bounds of the five ordinal ramp steps for switches in one hour. Fixed,
 *  so hours and days stay comparable. */
export const SWITCH_LEVELS = [1, 5, 10, 20, 40] as const;
/** Ramp step 1–5 for a switch count; 0 for none. */
export function switchLevel(count: number) {
  let level = 0;
  for (const minimum of SWITCH_LEVELS) if (count >= minimum) level++;
  return level;
}
export const switchLevelLabel = (level: number) => level >= SWITCH_LEVELS.length
  ? `${SWITCH_LEVELS[SWITCH_LEVELS.length - 1]}+` : `${SWITCH_LEVELS[level - 1]}–${SWITCH_LEVELS[level] - 1}`;
export const hourSwitches = (hour: HourlyFocus, kind: SwitchKind) =>
  kind === "category" ? hour.category_switches ?? 0 : hour.application_switches;

/** Stretch-length buckets, longest first, each with its ordinal ramp step. */
export const stretchBuckets: { key: "long" | "medium" | "short"; label: string; ramp: number }[] = [
  { key: "long", label: "25 min or more", ramp: 5 },
  { key: "medium", label: "10–25 min", ramp: 3 },
  { key: "short", label: "Under 10 min", ramp: 1 },
];
export const stretchDistribution = (metrics: FocusMetrics) => stretchBuckets.map(bucket => ({
  ...bucket, ...metrics.stretches[bucket.key],
  percent: metrics.active_ms > 0 ? 100 * metrics.stretches[bucket.key].elapsed_ms / metrics.active_ms : 0,
}));
export const plural = (count: number, singular: string, many = singular + "s") => `${count} ${count === 1 ? singular : many}`;
