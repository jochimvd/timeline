import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const load = async entry => {
  const built = await build({ entryPoints: [entry], bundle: true, write: false, format: "esm", platform: "browser" });
  return import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));
};
const routes = await load("web/routes.ts");
const trends = await load("web/trends.ts");
const dates = await load("web/dates.ts");
const categoryTree = await load("web/categories.ts");
const flat = names => categoryTree.categoryGrouping(names.map(name => ({ name, parent: null })), "category");

test("routes round-trip views, normalize periods and reject malformed dates", () => {
  assert.deepEqual(routes.parseRoute(""), { view: "day" });
  assert.deepEqual(routes.parseRoute("?date=2026-10-07"), { view: "day", date: "2026-10-07" });
  assert.deepEqual(routes.parseRoute("?view=week&date=2026-10-07"), { view: "week", date: "2026-10-05" }, "weeks start on Monday");
  assert.deepEqual(routes.parseRoute("?view=month&date=2026-10-07"), { view: "month", date: "2026-10-01" });
  assert.deepEqual(routes.parseRoute("?view=week&date=2026-02-30"), { view: "day" });
  assert.deepEqual(routes.parseRoute("?date=yesterday"), { view: "day" });
  assert.equal(routes.routeSearch({ view: "week", date: "2026-10-05" }), "?view=week&date=2026-10-05");
  assert.equal(routes.routeSearch({ view: "day" }), "/");
  const sunday = routes.period("week", "2026-10-11");
  assert.deepEqual(sunday, { kind: "week", first: "2026-10-05", last: "2026-10-11", length: 7 });
  assert.equal(routes.shiftPeriod(sunday, -1).first, "2026-09-28");
  assert.deepEqual(routes.previousSpan(sunday), { first: "2026-09-28", last: "2026-10-04" });
  const february = routes.period("month", "2028-02-10");
  assert.equal(february.length, 29, "leap year");
  assert.equal(routes.shiftPeriod(february, 1).first, "2028-03-01");
  assert.deepEqual(routes.previousSpan(routes.period("month", "2026-10-31")), { first: "2026-08-31", last: "2026-09-30" },
    "a month compares with as many days before it");
  assert.equal(routes.periodTitle(sunday), "Week of 5–11 Oct 2026");
  assert.equal(dates.dateSpan("2026-12-28", "2027-01-03"), "28 Dec 2026 – 3 Jan 2027");
  assert.equal(dates.dateRuns(["2026-09-21", "2026-09-23", "2026-09-24", "2026-09-25", "2026-09-30", "2026-10-01"]),
    "Mon 21 Sept, Wed 23 – Fri 25 Sept and Wed 30 Sept – Thu 1 Oct");
  assert.equal(dates.activityDate(Date.parse("2026-10-07T03:59:00Z"), "UTC"), "2026-10-06", "before 04:00 belongs to the previous day");
});

const HOUR = 3_600_000, MIN = 60_000;
const day = (date, { focus = 0, active = focus, categories = [], applications = [], observed = 0, gap = 0, switches = 0,
  longest = null, long = 0, hours = [], intervals = [] } = {}) => {
  const start_ms = Date.parse(date + "T04:00:00Z");
  return {
    date, range: { start_ms, end_ms: start_ms + 24 * HOUR },
    totals: { focus_ms: focus, no_focus_ms: 0, idle_ms: focus - active },
    categories, applications,
    evidence: { observed_ms: observed, gap_ms: gap, unknown_ms: 24 * HOUR - observed - gap, intervals },
    metrics: { active_ms: active, application_switches: switches, category_switches: categories.length ? switches : null,
      application_switches_per_hour: null, category_switches_per_hour: null,
      longest_stretch: longest && { range: { start_ms, end_ms: start_ms + longest }, elapsed_ms: longest, subject: { kind: "application", name: "editor" }, interruptions: 0 },
      stretches: { long: { count: long ? 1 : 0, elapsed_ms: long }, medium: { count: 0, elapsed_ms: 0 }, short: { count: 0, elapsed_ms: 0 } },
      hours },
  };
};
const app = (application, active_ms) => ({ application, focus_ms: active_ms, active_ms });

test("period summaries add days, keep the longest stretch and separate missing evidence from the future", () => {
  const days = [
    day("2026-10-05", { focus: 5 * HOUR, active: 4 * HOUR, observed: 8 * HOUR, switches: 20, longest: 40 * MIN, long: 2 * HOUR }),
    day("2026-10-06", { gap: 24 * HOUR }),
    day("2026-10-07", { focus: 2 * HOUR, observed: 3 * HOUR, switches: 4, longest: 50 * MIN }),
    day("2026-10-08"),
  ];
  const now = days[3].range.start_ms - 1;
  const summary = trends.summarize(days, now);
  assert.equal(summary.focus_ms, 7 * HOUR);
  assert.equal(summary.idle_ms, HOUR);
  assert.equal(trends.switchRate(summary, "application"), 4, "rates are recomputed from switches and active focus");
  assert.equal(summary.longest.date, "2026-10-07");
  assert.equal(trends.longShare(summary), 100 * 2 / 6);
  assert.deepEqual(summary.missing, ["2026-10-06"]);
  assert.equal(summary.elapsed_days, 3, "a day that has not begun is neither recorded nor missing");
  assert.deepEqual(days.map(value => trends.dayState(value, now)), ["recorded", "missing", "recorded", "future"]);
  assert.deepEqual(trends.durationDelta(90 * MIN, 30 * MIN, "vs last week"), { text: "+1h", direction: 1, label: "vs last week" });
  assert.equal(trends.durationDelta(30 * MIN + 10_000, 30 * MIN, "").text, "No change");
  assert.equal(trends.rateDelta(4, 5.5, "").text, "−1.5/h");
  assert.equal(trends.rateDelta(null, 5.5, ""), undefined);
});

test("subcategories roll up to their top-level category or stay in full detail", () => {
  const nodes = [{ name: "Work", parent: null }, { name: "Acme", parent: "Work" },
    { name: "Project X", parent: "Acme" }, { name: "Personal", parent: null }];
  assert.deepEqual(categoryTree.categoryPath(nodes, "Project X"), ["Work", "Acme", "Project X"]);
  assert.equal(categoryTree.categoryLabel(nodes, "Project X"), "Work › Acme › Project X");
  assert.equal(categoryTree.categoryLabel(nodes, null), "Uncategorized");
  assert.equal(categoryTree.grouping("subcategory", nodes), "subcategory");
  assert.equal(categoryTree.grouping("subcategory", [{ name: "Flat", parent: null }]), "category",
    "a remembered subcategory choice falls back once nothing is nested");
  assert.equal(categoryTree.grouping("category", []), "application");

  const rows = list => list.map(([category, active_ms]) => ({ category, focus_ms: active_ms, active_ms }));
  const days = [day("2026-10-05", { categories: rows([["Project X", 3 * HOUR], ["Work", HOUR], ["Personal", HOUR]]) })];
  const top = trends.subjects(days, "category", categoryTree.categoryGrouping(nodes, "category"));
  assert.deepEqual(top.map(subject => [subject.name, subject.total]), [["Work", 4 * HOUR], ["Personal", HOUR]]);
  const detail = trends.subjects(days, "category", categoryTree.categoryGrouping(nodes, "subcategory"));
  assert.deepEqual(detail.map(subject => [subject.name, subject.total]),
    [["Work", HOUR], ["Project X", 3 * HOUR], ["Personal", HOUR]], "a parent's own matches stay its own series");
  const previous = [day("2026-09-28", { categories: rows([["Acme", 2 * HOUR]]) })];
  const changes = trends.compareSubjects(days, previous, "category", categoryTree.categoryGrouping(nodes, "category"));
  assert.deepEqual(changes.map(row => [row.subject.name, row.current, row.previous]),
    [["Work", 4 * HOUR, 2 * HOUR], ["Personal", HOUR, 0]]);
});

test("subjects stack categories in configured order and fold the application tail", () => {
  const categories = (rows) => rows.map(([category, active_ms]) => ({ category, focus_ms: active_ms, active_ms }));
  const days = [
    day("2026-10-05", { categories: categories([["Code", 3 * HOUR], [null, HOUR]]) }),
    day("2026-10-06", { categories: categories([["Chat", HOUR]]) }),
  ];
  const stacked = trends.subjects(days, "category", flat(["Code", "Chat", "Unused"]));
  assert.deepEqual(stacked.map(subject => [subject.name, subject.days]),
    [["Code", [3 * HOUR, 0]], ["Chat", [0, HOUR]], ["Uncategorized", [HOUR, 0]]]);
  const previous = [day("2026-09-28", { categories: categories([["Unused", HOUR], ["Code", HOUR]]) })];
  const changes = trends.compareSubjects(days, previous, "category", flat(["Code", "Chat", "Unused"]));
  assert.deepEqual(changes.map(row => [row.subject.name, row.current, row.previous]),
    [["Code", 3 * HOUR, HOUR], ["Chat", HOUR, 0], ["Unused", 0, HOUR], ["Uncategorized", HOUR, 0]],
    "categories only seen in the comparison period stay listed in order");

  const many = [day("2026-10-05", { applications: Array.from({ length: 10 }, (_, index) => app("app" + index, (10 - index) * MIN)) })];
  const folded = trends.subjects(many, "application", flat([]), 7);
  assert.equal(folded.length, 8);
  assert.equal(folded.at(-1).key, trends.OTHER);
  assert.equal(folded.at(-1).name, "3 other applications");
  assert.deepEqual(folded.at(-1).days, [6 * MIN]);
  assert.equal(trends.subjects(many.map(value => ({ ...value, applications: value.applications.slice(0, 8) })), "application", flat([]), 7).length, 8,
    "one remaining application keeps its name");
  const before = [day("2026-09-28", { applications: [app("app0", MIN), app("elsewhere", 5 * MIN)] })];
  const rows = trends.compareSubjects(many, before, "application", flat([]), 7);
  assert.equal(rows[0].previous, MIN);
  assert.equal(rows.at(-1).previous, 5 * MIN, "other compares with applications not listed");
});

test("heatmap cells follow local hours and mark missing evidence and the future", () => {
  const start = Date.parse("2026-10-05T04:00:00Z");
  const value = day("2026-10-05", {
    observed: 2 * HOUR,
    intervals: [{ range: { start_ms: start + 5 * HOUR, end_ms: start + 7 * HOUR }, kind: "observed" }],
    hours: [{ range: { start_ms: start + 5 * HOUR, end_ms: start + 6 * HOUR }, active_ms: 30 * MIN, application_switches: 3, category_switches: 1 }],
  });
  const [row] = trends.heatmap([value], "category", "UTC", start + 6.5 * HOUR);
  assert.equal(row.cells.length, 24);
  assert.equal(row.cells[0].hour, 4, "activity days begin at 04:00");
  assert.equal(row.cells[5].hour, 9);
  assert.equal(row.cells[5].switches, 1, "switches follow the color mode");
  assert.equal(row.cells[5].active_ms, 30 * MIN);
  assert.deepEqual(row.cells.slice(4, 8).map(cell => cell.state), ["missing", "recorded", "recorded", "future"]);
  // A 25-hour day repeats a local hour; both combine in one cell.
  const long = { ...day("2026-10-24"), range: { start_ms: Date.parse("2026-10-24T02:00:00Z"), end_ms: Date.parse("2026-10-25T03:00:00Z") } };
  const [oslo] = trends.heatmap([long], "application", "Europe/Oslo", Infinity);
  assert.ok(oslo.cells.every(cell => cell !== null));
  assert.equal(oslo.cells[22].range.end_ms - oslo.cells[22].range.start_ms, 2 * HOUR, "02:00 occurs twice");
});
