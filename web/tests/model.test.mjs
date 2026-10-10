import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const built = await build({ entryPoints: ["web/model.ts"], bundle: true, write: false, format: "esm", platform: "browser" });
const { createTimelineApp } = await import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));
const deferred = () => {
  let resolve;
  const promise = new Promise(done => resolve = done);
  return { promise, resolve };
};
const day = date => {
  const start_ms = Date.parse(date + "T04:00:00Z");
  return { start_ms, end_ms: start_ms + 86_400_000 };
};
const overview = (range, application = "Editor") => ({
  resolved: { range, timezone: "UTC" }, revision: JSON.stringify(range),
  totals: { focus_ms: application ? 60_000 : 0, idle_ms: 0 },
  breakdown: { next_cursor: null, truncated: false, items: application ? [{
    kind: "display_focus", range: { start_ms: range.start_ms, end_ms: range.start_ms + 60_000 },
    focus: { kind: "focus", window: { application, title: application } },
  }] : [] },
});

test("date transitions retain a complete snapshot through resolution, paging, errors and obsolete responses", async () => {
  const original = { fetch: globalThis.fetch, document: globalThis.document, window: globalThis.window };
  globalThis.document = { removeEventListener() {} };
  globalThis.window = { removeEventListener() {} };
  const requests = [];
  let respond = (operation, args) => operation === "resolve_range"
    ? { range: day(args.date), timezone: "UTC" } : overview(args.range);
  // Commits follow each overview read; they name the range they were read for.
  let commits = args => ({ resolved: { range: args.range, timezone: "UTC" }, configured: true, commits: [], days: [], repositories: 1, problems: [], settings_problem: null });
  globalThis.fetch = async (url, options) => {
    const operation = url.split("/").at(-1), args = JSON.parse(options.body);
    requests.push({ operation, args });
    const result = await (operation === "commits" ? commits(args) : respond(operation, args));
    return result instanceof Response ? result : Response.json(result);
  };
  const app = createTimelineApp();
  let view;
  const unsubscribe = app.state.subscribe(value => view = value);
  try {
    await app.loadDay("2026-09-22");
    const first = view;
    assert.deepEqual(first.commits.resolved.range, first.loadedRange, "commits arrive with their rows");
    const resolveDay = deferred(), readDay = deferred(), readStarted = deferred();
    respond = (operation, args) => {
      if (operation === "resolve_range") return resolveDay.promise;
      readStarted.resolve();
      return readDay.promise;
    };
    const pending = app.loadDay("2026-09-23");
    assert.equal(view.rows, first.rows);
    assert.equal(view.visible, first.visible, "keep the old scale until its replacement is complete");
    assert.equal(view.dayLoading, true);
    resolveDay.resolve({ range: day("2026-09-23"), timezone: "UTC" });
    await readStarted.promise;
    assert.equal(view.rows, first.rows);
    assert.equal(view.visible, first.visible);
    // A paginated response is also staged until every row arrives.
    const nextPage = deferred(), pageStarted = deferred();
    respond = () => { pageStarted.resolve(); return nextPage.promise; };
    const replacement = overview(day("2026-09-23"), "Browser");
    readDay.resolve({ ...replacement, breakdown: { ...replacement.breakdown, next_cursor: "page2" } });
    await pageStarted.promise;
    assert.equal(view.blocks, first.blocks);
    assert.equal(view.totals, first.totals);
    // Unreadable commits leave the timeline usable and say why.
    commits = () => new Response(JSON.stringify({ error: "unavailable" }), { status: 404 });
    nextPage.resolve({ items: [], next_cursor: null, truncated: false });
    await pending;
    assert.equal(view.blocks[0].group.application, "Browser");
    assert.equal(view.commits, null);
    assert.equal(view.commitError, "unavailable");
    assert.deepEqual(view.visible, day("2026-09-23"));
    assert.equal(view.loadedRange, view.visible);
    assert.equal(view.rangeLoading, false);

    const loaded = view;
    respond = () => new Response(JSON.stringify({ error: "unavailable" }), { status: 503 });
    await app.loadDay("2026-09-24");
    assert.equal(view.date, "2026-09-23", "a failed date must not relabel the retained day");
    assert.equal(view.day, loaded.day);
    assert.equal(view.blocks, loaded.blocks);
    assert.equal(view.loadedRange, loaded.loadedRange);
    assert.equal(view.message, "unavailable");
    assert.equal(view.dayLoading, false);

    const oldResponse = deferred(), oldStarted = deferred();
    respond = (operation, args) => {
      if (operation === "resolve_range") return { range: day(args.date), timezone: "UTC" };
      if (args.range.start_ms === day("2026-09-24").start_ms) {
        oldStarted.resolve(); return oldResponse.promise;
      }
      return overview(args.range, "");
    };
    const obsolete = app.loadDay("2026-09-24");
    await oldStarted.promise;
    await app.loadDay("2026-09-25");
    assert.deepEqual(view.rows, [], "a resolved empty day must replace retained content");
    assert.deepEqual(view.blocks, []);
    oldResponse.resolve(overview(day("2026-09-24"), "Obsolete"));
    await obsolete;
    assert.equal(view.date, "2026-09-25");
    assert.deepEqual(view.blocks, [], "late obsolete data must not overwrite the newest date");

    const beforeRevisit = requests.length;
    await app.loadDay("2026-09-23");
    assert.deepEqual(requests.slice(beforeRevisit).map(r => r.operation), ["overview", "commits"],
      "revisiting a resolved date needs only its actual overview and commits");
    assert.ok(requests.filter(r => r.operation === "overview").every(r => r.args.range && !r.args.date),
      "day changes never scan a discarded overview just to resolve calendar boundaries");
  } finally {
    unsubscribe(); app.dispose();
    Object.assign(globalThis, original);
  }
});

test("selection commits labels with images and preserves displayed evidence on request failure", async () => {
  const original = { fetch: globalThis.fetch, document: globalThis.document, window: globalThis.window };
  globalThis.document = { removeEventListener() {} };
  globalThis.window = { removeEventListener() {} };
  const shot = (id, monitor, unavailable) => ({ screenshot: { id, monitor, unavailable, image: "image-" + id, width: 1, height: 1 }, stale: false });
  const firstShots = [shot(1, "A", "fixture_gap")], nextShots = [shot(2, "B", null)];
  const image = deferred(), imageStarted = deferred();
  let fail = false;
  globalThis.fetch = async (url, options) => {
    if (url.startsWith("/api/tiles/")) { imageStarted.resolve(); return image.promise; }
    const args = JSON.parse(options.body);
    if (fail) return new Response(JSON.stringify({ error: "unavailable" }), { status: 503 });
    return Response.json({ selected_ms: args.selected_ms, focus: [], screenshots: args.selected_ms === 1000 ? firstShots : nextShots });
  };
  const app = createTimelineApp();
  let view;
  const unsubscribe = app.state.subscribe(value => view = value);
  try {
    await app.select(1000);
    const displayed = view;
    const pending = app.select(2000);
    await imageStarted.promise;
    assert.equal(view.selected, 2000, "the cursor follows intent immediately");
    assert.equal(view.evidenceTime, 1000, "the heading must describe displayed evidence");
    assert.equal(view.shots, displayed.shots, "monitor controls must not switch ahead of their images");
    assert.equal(view.frames, displayed.frames);
    image.resolve(new Response(null, { status: 404 }));
    await pending;
    assert.equal(view.evidenceTime, 2000);
    assert.equal(view.shots[0].screenshot.monitor, "B");
    assert.match(view.frames[0].error, /404/, "a completed missing image is shown explicitly");
    const completed = view;
    fail = true;
    await app.select(3000);
    assert.equal(view.frames, completed.frames);
    assert.equal(view.shots, completed.shots);
    assert.equal(view.evidenceTime, completed.evidenceTime);
    assert.match(view.evidenceError, /Previous evidence remains displayed/);
    assert.equal(view.evidenceLoading, false);
  } finally {
    unsubscribe(); app.dispose(); Object.assign(globalThis, original);
  }
});

test("display blocks carry and split on categories; category rows keep the server's tree order", async () => {
  const original = { fetch: globalThis.fetch, document: globalThis.document, window: globalThis.window };
  globalThis.document = { removeEventListener() {} };
  globalThis.window = { removeEventListener() {} };
  const range = day("2026-09-22");
  const focus = (start, end, title, category) => ({ kind: "display_focus", range: { start_ms: range.start_ms + start, end_ms: range.start_ms + end },
    segment: 1, recording_boundary: 1, focus: { kind: "focus", window: { application: "Editor", title }, category } });
  const category = (name, elapsed_ms, extra = {}) => ({ kind: "category", category: name, elapsed_ms, percent_of_focus: elapsed_ms / 600, ...extra });
  globalThis.fetch = async (url, options) => {
    const operation = url.split("/").at(-1), args = JSON.parse(options.body);
    if (operation === "resolve_range") return Response.json({ range: day(args.date), timezone: "UTC" });
    return Response.json({ resolved: { range: args.range, timezone: "UTC" }, revision: "r", totals: { focus_ms: 60_000, idle_ms: 0 },
      breakdown: { next_cursor: null, truncated: false, items: [
        category("Unused", 0, { palette: 2 }), category(null, 20_000), category("Planning", 20_000, { palette: 1 }), category("Review", 20_000, { color: "#4f8cff" }),
        focus(0, 10_000, "plan.md", "Planning"), focus(10_000, 20_000, "plan.md", "Review"), focus(20_000, 30_000, "notes", null),
      ] } });
  };
  const app = createTimelineApp();
  let view;
  const unsubscribe = app.state.subscribe(value => view = value);
  try {
    await app.loadDay("2026-09-22");
    assert.deepEqual(view.blocks.map(block => block.category), ["Planning", "Review", null],
      "a category change splits otherwise identical display blocks");
    assert.deepEqual(view.categories.map(row => row.category), ["Unused", null, "Planning", "Review"],
      "views rank categories themselves, so the tree order survives");
  } finally {
    unsubscribe(); app.dispose(); Object.assign(globalThis, original);
  }
});
