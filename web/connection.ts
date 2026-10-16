import { get, readonly, writable } from "svelte/store";

/** Why the page cannot read from the server: its session ended, its launch
 *  link was already spent, or the server is not answering. */
export type Lost = "expired" | "link_used" | "unreachable";
export const lostLabel: Record<Lost, string> = {
  expired: "Session expired", link_used: "Link already used", unreachable: "Disconnected",
};

const store = writable<Lost | null>(null);
/** Null while connected. The page shows one signed-out state for every request
 *  it affects, so no caller reports it on its own. */
export const connection = readonly(store);

/** A request refused for want of a session, or that never reached the server. */
export class SignedOut extends Error {
  constructor(readonly reason: Lost) { super(lostLabel[reason] + "."); }
}
export function lose(reason: Lost) {
  // A spent link is why the session is missing; later refusals say no more.
  if (!(reason === "expired" && get(store) === "link_used")) store.set(reason);
}
export function restore() {
  if (get(store)) store.set(null);
}
