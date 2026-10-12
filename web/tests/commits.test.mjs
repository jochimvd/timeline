import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const built = await build({ entryPoints: ["web/commits.ts"], bundle: true, write: false, format: "esm", platform: "browser" });
const commits = await import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));
const commit = (time_ms, repository = "work/timeline", hash = String(time_ms)) =>
  ({ repository, hash, short_hash: hash.slice(0, 7), subject: "s", branch: "main", time_ms, timing: "reflog", kind: "commit", amends: 0 });

test("commits that would overlap share a marker, and only those in view are drawn", () => {
  const range = { start_ms: 0, end_ms: 1000 };
  // 1000 ms over 100 px: 10 ms per pixel, so neighbours within 140 ms cluster.
  const clusters = commits.clusterCommits([commit(-5), commit(0), commit(100), commit(300), commit(420), commit(999), commit(1000)], range, 100);
  assert.deepEqual(clusters.map(cluster => cluster.commits.map(c => c.time_ms)), [[0, 100], [300, 420], [999]]);
  assert.deepEqual(clusters.map(cluster => [cluster.start_ms, cluster.end_ms]), [[0, 100], [300, 420], [999, 999]]);
  assert.deepEqual(commits.clusterCommits([commit(0), commit(100)], range, 1000).length, 2, "zooming in separates them");
  assert.deepEqual(commits.clusterCommits([commit(0)], range, 0), []);
});

test("commits group by activity day and repository", () => {
  const day = (date, start_ms, count) => ({ date, range: { start_ms, end_ms: start_ms + 100 }, commits: count });
  const list = { commits: [commit(10, "a/timeline"), commit(20, "b/notes"), commit(30, "a/timeline"), commit(150, "a/timeline")],
    days: [day("2026-09-22", 0, 3), day("2026-09-23", 100, 1), day("2026-09-24", 200, 0)], problems: [] };
  const days = commits.commitDays(list);
  assert.deepEqual([...days].map(([date, value]) => [date, value.count, value.commits.length]),
    [["2026-09-22", 3, 3], ["2026-09-23", 1, 1], ["2026-09-24", 0, 0]]);
  assert.deepEqual(commits.byRepository(days.get("2026-09-22").commits), [["a/timeline", 2], ["b/notes", 1]]);
  assert.equal(commits.commitCount(1), "1 commit");
  assert.equal(commits.commitProblems({ ...list, problems: [{ repository: "x", problem: "y" }] }), "1 repository or path could not be read");
  assert.equal(commits.commitProblems(null), "");
});
