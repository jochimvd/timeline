import { writable } from "svelte/store";

/** What timeline fills, lanes, and week columns are grouped by. Without
 *  configured categories, views fall back to applications. */
export type ColorBy = "application" | "category";
const key = "timeline.colorBy";
function stored(): ColorBy {
  try { return localStorage.getItem(key) === "category" ? "category" : "application"; } catch { return "application"; }
}
/** Remembered per browser. */
export const colorBy = writable<ColorBy>(stored());
colorBy.subscribe(value => { try { localStorage.setItem(key, value); } catch {} });
