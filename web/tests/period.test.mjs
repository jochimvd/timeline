import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const built = await build({ entryPoints: ["web/period.ts", "web/routes.ts"], bundle: true, write: false, format: "esm", platform: "browser", outdir: "out", splitting: false });
const load = file => import("data:text/javascript;base64," + Buffer.from(built.outputFiles.find(output => output.path.endsWith(file)).text).toString("base64"));
const { createPeriodModel } = await load("period.js");
const { period } = await load("routes.js");
const HOUR = 3_600_000;
const dayRange = date => { const start_ms = Date.parse(date + "T04:00:00Z"); return { start_ms, end_ms: start_ms + 24 * HOUR }; };
const metrics = (dates, range) => ({
  resolved: { range: range ?? { start_ms: dayRange(dates[0]).start_ms, end_ms: dayRange(dates.at(-1)).end_ms }, timezone: "UTC" },
  days: dates.map(date => ({ date, range: dayRange(date) })),
});
const datesFrom = (first, count) => Array.from({ length: count }, (_, index) => {
  const value = new Date(first + "T12:00:00Z"); value.setUTCDate(value.getUTCDate() + index); return value.toISOString().slice(0, 10);
});

test("an in-progress period compares with the same activity day and time of the previous one", async () => {
  const original = { fetch: globalThis.fetch, now: Date.now };
  const requests = [];
  Date.now = () => Date.parse("2026-10-07T10:30:00Z"); // Wednesday, 6.5 h into its activity day
  globalThis.fetch = async (url, options) => {
    const operation = url.split("/").at(-1), args = JSON.parse(options.body);
    requests.push({ operation, args });
    if (operation === "categories") return Response.json({ categories: [], problem: null });
    if (operation === "commits") return Response.json({ configured: false, commits: [] });
    if (operation === "sessions") return Response.json({ sessions: [{ range: dayRange(args.dates.first) }] });
    if (operation === "resolve_range") return Response.json({ range: dayRange(args.date), timezone: "UTC" });
    if (args.dates) return Response.json(metrics(datesFrom(args.dates.first, 7)));
    return Response.json(metrics(datesFrom("2026-09-28", 3), args.range));
  };
  const model = createPeriodModel();
  let state;
  const shown = [];
  const unsubscribe = model.state.subscribe(value => {
    if (value.current && value.current !== state?.current) shown.push(value);
    state = value;
  });
  try {
    await model.load(period("week", "2026-10-07"));
    const comparison = requests.filter(request => request.operation === "focus_metrics").at(-1).args;
    assert.deepEqual(comparison.range, { start_ms: Date.parse("2026-09-28T04:00:00Z"), end_ms: Date.parse("2026-09-30T10:30:00Z") });
    assert.equal(state.comparison.partial, true);
    assert.equal(state.comparison.last, "2026-09-30");
    assert.equal(state.current.days.length, 7);
    // The week shows once, with its sessions, rather than filling in.
    assert.equal(shown.length, 1);
    assert.equal(shown[0].shown.first, "2026-10-05");
    assert.equal(shown[0].sessions.length, 1);

    // A past period compares with the whole previous one, by dates.
    requests.length = 0;
    await model.load(period("week", "2026-09-21"));
    assert.deepEqual(requests.filter(request => request.operation === "focus_metrics").map(request => request.args),
      [{ dates: { first: "2026-09-21", last: "2026-09-27" } }, { dates: { first: "2026-09-14", last: "2026-09-20" } }]);
    assert.equal(state.comparison.partial, false);

    // An older request finishing late never replaces a newer period.
    const held = [];
    globalThis.fetch = async (url, options) => {
      const args = JSON.parse(options.body);
      if (args.dates?.first === "2026-08-03") await new Promise(resolve => held.push(resolve));
      if (url.endsWith("categories")) return Response.json({ categories: [], problem: null });
      return Response.json(metrics(datesFrom(args.dates?.first ?? "2026-08-31", 7)));
    };
    const obsolete = model.load(period("week", "2026-08-05"));
    await model.load(period("week", "2026-09-08"));
    held.forEach(release => release());
    await obsolete;
    assert.equal(state.period.first, "2026-09-07");
    assert.equal(state.current.days[0].date, "2026-09-07");
  } finally {
    unsubscribe(); model.dispose();
    globalThis.fetch = original.fetch; Date.now = original.now;
  }
});
