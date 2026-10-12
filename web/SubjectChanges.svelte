<script lang="ts">
  import { duration } from "./format";
  import type { Painter } from "./paint";
  import { compareSubjects, durationDelta } from "./trends";
  import type { DayFocusMetrics } from "./types";
  /** Where active focus went, against the comparison period. */
  let { days, previous, paint, comparing, current, name }: {
    days: DayFocusMetrics[]; previous: DayFocusMetrics[] | null; paint: Painter; comparing: boolean;
    /** "This week so far"; `name` names the comparison, such as "last week". */
    current: string; name: string;
  } = $props();
  const rows = $derived(compareSubjects(days, previous, paint.mode, paint.grouping));
  const max = $derived(Math.max(1, ...rows.flatMap(row => [row.current, row.previous ?? 0])));
  const total = $derived(rows.reduce((sum, row) => sum + row.current, 0));
</script>

<section class="changes card" aria-labelledby="changes-heading">
  <header>
    <h2 id="changes-heading" class="card-title">{paint.mode === "category" ? "Categories" : "Applications"}</h2>
    <span class="muted">active focus{previous ? ` · tick: ${name}` : comparing ? " · comparing…" : ""}</span>
  </header>
  {#if total > 0 || rows.length}
    <ul id="subject-changes">
      {#each rows as row (row.subject.key)}
        {@const fill = paint.subject(row.subject).fill}
        {@const delta = row.previous === null ? undefined : durationDelta(row.current, row.previous, `vs ${name}`)}
        <li>
          <div class="line">
            <span class="dot" style:background={fill}></span>
            <span class="name ellipsis" title={row.subject.name}>{row.subject.name}</span>
            <span class="value num">{duration(row.current) || "—"}</span>
            {#if delta}<span class="delta num" title={delta.label}>{delta.direction > 0 ? "↑" : delta.direction < 0 ? "↓" : "·"} {delta.direction ? delta.text.slice(1) : ""}</span>{/if}
          </div>
          <div class="bar" role="img" aria-label={`${current} ${duration(row.current) || "none"}${row.previous !== null ? `, ${name} ${duration(row.previous) || "none"}` : ""}`}>
            <span class="fill" style:width={100 * row.current / max + "%"} style:background={fill}></span>
            {#if row.previous !== null}<span class="tick" style:left={100 * row.previous / max + "%"}></span>{/if}
          </div>
        </li>
      {/each}
    </ul>
  {:else}<p class="muted">No active focus yet.</p>{/if}
</section>

<style>
  .changes { padding: 14px 16px; display: flex; flex-direction: column; gap: 12px; }
  header { display: flex; align-items: baseline; justify-content: space-between; gap: 10px; font-size: 12px; }
  ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 11px; }
  .line { display: flex; align-items: center; gap: 8px; font-size: 13px; }
  .name { flex: 1; }
  .value { font-size: 12.5px; }
  .delta { font-size: 11.5px; color: var(--muted); min-width: 58px; text-align: right; }
  .bar { position: relative; height: 6px; margin: 5px 0 0 16px; border-radius: 3px; background: var(--surface-3); }
  .fill { display: block; height: 100%; border-radius: 3px; }
  .tick { position: absolute; top: -3px; width: 2px; height: 12px; margin-left: -1px; border-radius: 1px; background: var(--text); }
</style>
