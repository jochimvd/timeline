export type Range = { start_ms: number; end_ms: number };
export type Block = {
  id: string;
  range: Range;
  group: { application: string; title: string };
  /** Configured category of the block's focus; null when uncategorized. */
  category: string | null;
  idle_ms: number;
  mixed?: boolean;
};
export type Coverage = {
  kind: string;
  mixed?: boolean;
  has_gap?: boolean;
  idle?: boolean;
  range?: Range;
  segment?: number;
  recording_boundary?: number;
  focus?: {
    kind: string;
    window?: { application: string; title: string };
    category?: string | null;
    idle?: boolean;
    reason?: string;
  };
  monitor?: string;
  unavailable?: string;
  application?: string;
  title?: string;
  /** Category of an exact-title or category row; null for uncategorized focus. */
  category?: string | null;
  /** Category rows: explicit `#rrggbb` color, else a 1-based palette slot. */
  color?: string | null;
  palette?: number | null;
  elapsed_ms?: number;
  percent_of_focus?: number;
};
export type Page<T> = { items: T[]; next_cursor: string | null; truncated: boolean };
/** What a focus stretch stays on: a category, or an uncategorized application. */
export type FocusSubject = { kind: "category" | "application"; name: string };
export type FocusStretch = { range: Range; elapsed_ms: number; subject: FocusSubject; interruptions: number };
export type StretchBucket = { count: number; elapsed_ms: number };
export type HourlyFocus = { range: Range; active_ms: number; application_switches: number; category_switches: number | null };
/** Context switches and focus stretches of a range; see README "Focus metrics". */
export type FocusMetrics = {
  active_ms: number;
  application_switches: number;
  category_switches: number | null;
  application_switches_per_hour: number | null;
  category_switches_per_hour: number | null;
  longest_stretch: FocusStretch | null;
  stretches: { long: StretchBucket; medium: StretchBucket; short: StretchBucket };
  hours: HourlyFocus[];
};
export type Overview = {
  revision: string;
  resolved: { range: Range; timezone: string };
  totals: { focus_ms: number; idle_ms: number };
  metrics: FocusMetrics;
  breakdown: Page<Coverage>;
};
export type Shot = {
  screenshot: {
    id: number;
    unix_ms: number;
    monitor: string;
    width: number;
    height: number;
    transform: number;
    image: string | null;
    unavailable: string | null;
  };
  stale: boolean;
};

export type Evidence = { selected_ms: number; focus: Coverage[]; screenshots: Shot[] };

export type OverviewUpdates = {
  revision: string; reset: boolean; totals: Overview["totals"] | null; metrics: FocusMetrics | null;
  changes: Page<{ remove: boolean; row: Coverage }>;
};

/** A configured category's legend entry, from overview `category` rows. */
export type CategoryRow = Coverage & { kind: "category"; category: string | null; elapsed_ms: number; percent_of_focus: number };
export type CategoryRule = { application?: string; title?: string };
export type CategoryList = {
  categories: { name: string; color: string | null; palette: number | null; rules: CategoryRule[] }[];
  problem: string | null;
};

/** Per-day focus by category or application; see README "Day evidence". */
export type CategoryFocus = { category: string | null; focus_ms: number; active_ms: number };
export type ApplicationFocus = { application: string; focus_ms: number; active_ms: number };
export type EvidenceKind = "observed" | "gap" | "unknown";
export type DayEvidence = {
  observed_ms: number; gap_ms: number; unknown_ms: number;
  intervals: { range: Range; kind: EvidenceKind; reason?: string }[];
};
export type DayFocusMetrics = {
  date: string; range: Range;
  totals: { focus_ms: number; no_focus_ms: number; idle_ms: number };
  categories: CategoryFocus[]; applications: ApplicationFocus[];
  evidence: DayEvidence; metrics: FocusMetrics;
};
export type FocusMetricsByDay = { resolved: { range: Range; timezone: string }; days: DayFocusMetrics[] };

/** One of the user's git commits; see README "Git commits". */
export type Commit = {
  repository: string; hash: string; short_hash: string; subject: string; branch: string | null;
  time_ms: number; timing: "reflog" | "committer_date";
  kind: "commit" | "merge" | "cherry_pick" | "revert" | "patch";
  /** How often it was amended; `first_ms` is when the original was made. */
  amends: number; first_ms?: number;
};
export type RepositoryProblem = { repository: string; problem: string };
export type CommitList = {
  resolved: { range: Range; timezone: string };
  /** Whether any git paths are configured. */
  configured: boolean;
  commits: Commit[];
  days: { date: string; range: Range; commits: number }[];
  repositories: number;
  /** Repositories whose commits are missing because they could not be read. */
  problems: RepositoryProblem[];
  settings_problem: string | null;
};
export type RepositoryList = {
  paths: string[]; authors: string[]; depth: number;
  repositories: { name: string; path: string }[];
  problems: RepositoryProblem[];
  scanned_ms: number | null; directories: number; truncated: boolean;
  settings_problem: string | null;
};
