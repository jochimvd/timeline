import { readonly, writable } from "svelte/store";

/** Brief feedback fades after a few seconds; an error stays until dismissed,
 *  superseded or cleared by its source succeeding. */
export type Toast = { id: number; key: string; text: string; tone: "info" | "error"; retry?: () => void };

const store = writable<Toast[]>([]);
const timers = new Map<string, ReturnType<typeof setTimeout>>();
let next = 0;
export const toasts = readonly(store);

/** Shows `text`, replacing the toast with the same `key` (by default the text
 *  itself), so a repeated message restarts instead of stacking. */
export function notify(text: string, { key = text, tone = "info", retry }: { key?: string; tone?: Toast["tone"]; retry?: () => void } = {}) {
  clearTimeout(timers.get(key));
  timers.delete(key);
  const toast: Toast = { id: ++next, key, text, tone, retry };
  store.update(list => [...list.filter(item => item.key !== key), toast].slice(-3));
  if (tone === "info") timers.set(key, setTimeout(() => dismiss(key, toast.id), 3_000));
}
/** Removes the toast for `key`, or only the given one of it. */
export function dismiss(key: string, id?: number) {
  clearTimeout(timers.get(key));
  timers.delete(key);
  store.update(list => list.some(item => item.key === key && (id === undefined || item.id === id))
    ? list.filter(item => item.key !== key || (id !== undefined && item.id !== id)) : list);
}
