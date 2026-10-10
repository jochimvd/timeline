import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const built = await build({ entryPoints: ["web/metrics.ts"], bundle: true, write: false, format: "esm", platform: "browser" });
const metrics = await import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));

test("hourly switch counts map to fixed ordinal steps", () => {
  const { switchLevel, switchLevelLabel, SWITCH_LEVELS } = metrics;
  assert.deepEqual([0, 1, 4, 5, 9, 10, 19, 20, 39, 40, 400].map(switchLevel), [0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5]);
  assert.deepEqual(SWITCH_LEVELS.map((_, index) => switchLevelLabel(index + 1)), ["1–4", "5–9", "10–19", "20–39", "40+"]);
});

test("switch kind selects application or category counts", () => {
  const hour = { range: { start_ms: 0, end_ms: 1 }, active_ms: 1, application_switches: 7, category_switches: null };
  assert.equal(metrics.hourSwitches(hour, "application"), 7);
  assert.equal(metrics.hourSwitches(hour, "category"), 0);
});

test("stretch distribution lists long stretches first with shares of active focus", () => {
  const bucket = (count, elapsed_ms) => ({ count, elapsed_ms });
  const rows = metrics.stretchDistribution({
    active_ms: 4_000, stretches: { long: bucket(1, 2_000), medium: bucket(0, 0), short: bucket(3, 2_000) },
  });
  assert.deepEqual(rows.map(row => [row.key, row.count, row.percent, row.ramp]),
    [["long", 1, 50, 5], ["medium", 0, 0, 3], ["short", 3, 50, 1]]);
  assert.equal(metrics.stretchDistribution({ active_ms: 0, stretches: { long: bucket(0, 0), medium: bucket(0, 0), short: bucket(0, 0) } })[0].percent, 0);
  assert.equal(metrics.plural(1, "stretch", "stretches"), "1 stretch");
  assert.equal(metrics.plural(2, "switch", "switches"), "2 switches");
});
