import { writable } from "svelte/store";

/** What timeline fills, lanes, and week columns are grouped by: applications,
 *  top-level categories, or categories in full detail. Without configured
 *  categories, views fall back to applications. */
export type ColorBy = "application" | "category" | "subcategory";
const key = "timeline.colorBy";
const choices: ColorBy[] = ["application", "category", "subcategory"];
function stored(): ColorBy {
  try { return choices.find(choice => choice === localStorage.getItem(key)) ?? "application"; } catch { return "application"; }
}
/** Remembered per browser. */
export const colorBy = writable<ColorBy>(stored());
colorBy.subscribe(value => { try { localStorage.setItem(key, value); } catch {} });
