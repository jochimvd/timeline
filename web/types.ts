export type Range = { start_ms: number; end_ms: number };
export type Block = {
  id: string;
  range: Range;
  group: { application: string; title: string };
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
    idle?: boolean;
    reason?: string;
  };
  monitor?: string;
  unavailable?: string;
  application?: string;
  title?: string;
  elapsed_ms?: number;
  percent_of_focus?: number;
};
export type Page<T> = { items: T[]; next_cursor: string | null; truncated: boolean };
export type Overview = {
  revision: string;
  resolved: { range: Range; timezone: string };
  totals: { focus_ms: number; idle_ms: number };
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
  revision: string; reset: boolean; totals: Overview["totals"] | null;
  changes: Page<{ remove: boolean; row: Coverage }>;
};
