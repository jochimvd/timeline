import type { Page, Overview } from "./types";
const tabId = crypto.randomUUID();
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
      reason === "unauthorized"
        ? "Session expired. Run timeline open to reconnect."
        : reason,
    );
  }
  return response.json();
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
export async function pages<T>(
  operation: string,
  args: object,
  signal: AbortSignal,
  first?: Page<T>,
): Promise<T[]> {
  let page =
      first ||
      (await query<Page<T>>(operation, { ...args, limit: 500 }, signal)),
    result: T[] = [],
    bytes = 0;
  for (;;) {
    bytes += new TextEncoder().encode(JSON.stringify(page)).byteLength;
    if (bytes > 16 * 1024 * 1024 || result.length + page.items.length > 20000)
      throw new Error(
        "Visible range exceeds display limit. Zoom in to inspect it.",
      );
    result.push(...page.items);
    if (!page.next_cursor) return result;
    const next = await query<Page<T> | Overview>(
      operation,
      { ...args, limit: 500, cursor: page.next_cursor },
      signal,
    );
    page = "breakdown" in next ? (next.breakdown as Page<T>) : next;
  }
}
