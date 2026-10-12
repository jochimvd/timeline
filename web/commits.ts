import { plural } from "./metrics";
import type { Commit, CommitList, Range } from "./types";

/** Commits closer than this many pixels share one timeline marker. */
export const CLUSTER_PX = 14;

/** Commits drawn as one marker: they would overlap at the current scale. */
export type CommitCluster = { key: string; start_ms: number; end_ms: number; commits: Commit[] };

/** Groups commits in `range` whose neighbours are within `gapPx` at `widthPx`. */
export function clusterCommits(commits: Commit[], range: Range, widthPx: number, gapPx = CLUSTER_PX): CommitCluster[] {
  if (widthPx <= 0) return [];
  const gap = gapPx * (range.end_ms - range.start_ms) / widthPx;
  const clusters: CommitCluster[] = [];
  for (const commit of commits) {
    if (commit.time_ms < range.start_ms || commit.time_ms >= range.end_ms) continue;
    const last = clusters.at(-1);
    if (last && commit.time_ms - last.end_ms <= gap) {
      last.commits.push(commit);
      last.end_ms = commit.time_ms;
    } else clusters.push({ key: commit.repository + "\n" + commit.hash, start_ms: commit.time_ms, end_ms: commit.time_ms, commits: [commit] });
  }
  return clusters;
}

/** "3 commits" or "1 commit". */
export const commitCount = (count: number) => plural(count, "commit");

/** The last path component of a repository name, which usually names the project. */
export const repositoryLabel = (name: string) => name.split("/").filter(Boolean).at(-1) ?? name;

/** Commits per repository, most first, then by name. */
export function byRepository(commits: Commit[]): [string, number][] {
  const counts = new Map<string, number>();
  for (const commit of commits) counts.set(commit.repository, (counts.get(commit.repository) ?? 0) + 1);
  return [...counts].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
}

/** Commits per activity date: the count and the commits themselves. */
export type CommitDay = { count: number; commits: Commit[] };
export function commitDays(list: CommitList): Map<string, CommitDay> {
  const days = new Map<string, CommitDay>(list.days.map(day => [day.date, { count: day.commits, commits: [] }]));
  for (const commit of list.commits) {
    const day = list.days.find(day => day.range.start_ms <= commit.time_ms && commit.time_ms < day.range.end_ms);
    if (day) days.get(day.date)!.commits.push(commit);
  }
  return days;
}

/** A short status of why commits are incomplete, if they are. */
export function commitProblems(list: CommitList | null) {
  const count = list?.problems.length ?? 0;
  return count ? `${count} ${count === 1 ? "repository or path" : "repositories or paths"} could not be read` : "";
}
