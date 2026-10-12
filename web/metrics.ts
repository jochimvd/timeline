import type { HourlyFocus } from "./types";

/** Which switches a view shows: between applications or between categories. */
export type SwitchKind = "application" | "category";

/** Lower bounds, in minutes, of steps 2–5 for active focus in one hour; any
 *  active focus is at least step 1. */
export const ACTIVE_LEVELS = [10, 20, 35, 50] as const;
/** Ramp step 1–5 of `value` on fixed lower bounds; 0 below the first. */
export function level(value: number, bounds: readonly number[]) {
  let step = 0;
  for (const minimum of bounds) if (value >= minimum) step++;
  return step;
}
/** Ramp step 1–5 of active focus within one hour; 0 for none. */
export const activeLevel = (ms: number) => ms > 0 ? 1 + level(Math.floor(ms / 60_000), ACTIVE_LEVELS) : 0;
export const activeLevelLabel = (step: number) => step === 1 ? `Under ${ACTIVE_LEVELS[0]} min`
  : step > ACTIVE_LEVELS.length ? `${ACTIVE_LEVELS[step - 2]}–60 min` : `${ACTIVE_LEVELS[step - 2]}–${ACTIVE_LEVELS[step - 1] - 1} min`;
/** Switches per hour of active focus, to one decimal like the server. */
export const perHour = (switches: number, activeMs: number) =>
  activeMs > 0 ? Math.round(switches * 3_600_000 / activeMs * 10) / 10 : null;
export const hourSwitches = (hour: HourlyFocus, kind: SwitchKind) =>
  kind === "category" ? hour.category_switches ?? 0 : hour.application_switches;

export const plural = (count: number, singular: string, many = singular + "s") => `${count} ${count === 1 ? singular : many}`;
