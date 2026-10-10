import type { Page, Overview } from "./types";
const tabId = crypto.randomUUID();
// Received JSON length per decoded response; bounds pagination without re-serializing.
const responseLength = new WeakMap<object, number>();
export async function request<T = unknown>(
  path: string,
  body?: unknown,
  signal?: AbortSignal,
  selectionGeneration?: number,
  channel = "selected",
): Promise<T> {
  const response = await fetch(path, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      "Content-Type": "application/json", "X-Timeline-Request": "1",
      ...(path === "/api/query/selection" ? { "X-Timeline-Selection": `${tabId}-${channel}:${selectionGeneration}` } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal,
  });
  if (!response.ok) {
    let reason = `Request failed (${response.status})`;
    try {
      reason = (await response.json()).error || reason;
    } catch {}
    throw new Error(
      reason === "unauthorized" ? "Session expired. Run timeline open to reconnect."
      : reason === "invalid_bootstrap" ? "This launch link was already used or has expired. Run timeline open for a new one."
      : reason,
    );
  }
  const text = await response.text();
  const value = JSON.parse(text);
  if (value !== null && typeof value === "object") responseLength.set(value, text.length);
  return value;
}
export async function query<T>(
  operation: string,
  args: object,
  signal: AbortSignal,
  selectionGeneration?: number,
  channel = "selected",
): Promise<T> {
  // An aborted server read may still be finishing. Retry admission, never queue server work.
  for (let attempt = 0; ; attempt++) {
    try {
      return await request<T>(`/api/query/${operation}`, args, signal, selectionGeneration, channel);
    } catch (e) {
      if (!(e instanceof Error) || e.message !== "busy" || attempt === 8)
        throw e;
      await new Promise((resolve) => setTimeout(resolve, 75));
      signal.throwIfAborted();
    }
  }
}
/** All items of a listing, or of an overview's breakdown, from its first response. */
export async function pages<T>(
  operation: string,
  args: object,
  signal: AbortSignal,
  first?: Page<T> | Overview,
): Promise<T[]> {
  let response =
      first ||
      (await query<Page<T>>(operation, { ...args, limit: 500 }, signal)),
    result: T[] = [],
    length = 0;
  for (;;) {
    const page = "breakdown" in response ? (response.breakdown as Page<T>) : response;
    length += responseLength.get(response) ?? 0;
    if (length > 16 * 1024 * 1024 || result.length + page.items.length > 20000)
      throw new Error(
        "Visible range exceeds display limit. Zoom in to inspect it.",
      );
    result.push(...page.items);
    if (!page.next_cursor) return result;
    response = await query<Page<T> | Overview>(
      operation,
      { ...args, limit: 500, cursor: page.next_cursor },
      signal,
    );
  }
}
