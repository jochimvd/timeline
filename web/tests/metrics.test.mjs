import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const built = await build({ entryPoints: ["web/metrics.ts"], bundle: true, write: false, format: "esm", platform: "browser" });
const metrics = await import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));

test("hourly active focus maps to fixed ordinal steps", () => {
  const { activeLevel, activeLevelLabel } = metrics;
  assert.deepEqual([0, 1, 9 * 60_000, 10 * 60_000, 35 * 60_000, 50 * 60_000, 60 * 60_000].map(activeLevel), [0, 1, 1, 2, 4, 5, 5]);
  assert.deepEqual([1, 2, 3, 4, 5].map(activeLevelLabel), ["Under 10 min", "10–19 min", "20–34 min", "35–49 min", "50–60 min"]);
});

test("switch kind selects application or category counts", () => {
  const hour = { range: { start_ms: 0, end_ms: 1 }, active_ms: 1, application_switches: 7, category_switches: null };
  assert.equal(metrics.hourSwitches(hour, "application"), 7);
  assert.equal(metrics.hourSwitches(hour, "category"), 0);
});

test("rates and counts read naturally", () => {
  assert.equal(metrics.perHour(3, 1_800_000), 6);
  assert.equal(metrics.perHour(1, 0), null);
  assert.equal(metrics.plural(1, "stretch", "stretches"), "1 stretch");
  assert.equal(metrics.plural(2, "switch", "switches"), "2 switches");
});
