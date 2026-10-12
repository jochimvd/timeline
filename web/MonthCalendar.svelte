<script lang="ts">
  import { commitCount, type CommitDay } from "./commits";
  import { dateLabel, weekday } from "./dates";
  import { duration } from "./format";
  import type { Painter } from "./paint";
  import { dayState, subjects } from "./trends";
  import type { DayFocusMetrics, Range } from "./types";
  /** A month as a calendar: each day's focus, its composition and commits. */
  let { days, paint, now, today, commits, onopen }: {
    days: DayFocusMetrics[]; paint: Painter; now: number; today: string;
    commits?: Map<string, CommitDay>; onopen: (date: string, range?: Range) => void;
  } = $props();
  const series = $derived(subjects(days, paint.mode, paint.grouping, 6));
  const max = $derived(Math.max(1, ...days.map(day => day.metrics.active_ms)));
  const lead = $derived(days.length ? weekday(days[0].date) : 0);
  const trail = $derived((7 - (lead + days.length) % 7) % 7);
  const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
  /** Tint by active focus, receding to the surface near zero. */
  const tint = (ms: number) => ms > 0 ? `color-mix(in oklab, var(--seq-3) ${Math.round(4 + 22 * ms / max)}%, var(--surface))` : undefined;
</script>

<section class="month card" aria-label="Month calendar">
  <div class="weekdays" aria-hidden="true">{#each WEEKDAYS as name (name)}<span>{name}</span>{/each}</div>
  <div class="cells">
    {#each Array(lead) as _, index (index)}<div class="blank"></div>{/each}
    {#each days as day, index (day.date)}
      {@const state = dayState(day, now)}
      {@const count = commits?.get(day.date)?.count ?? 0}
      <button class="cell" class:today={day.date === today} class:missing={state === "missing"} class:future={state === "future"}
        class:hatched={state === "missing"} style:background-color={state === "recorded" ? tint(day.metrics.active_ms) : undefined}
        onclick={() => onopen(day.date)} aria-label={`${dateLabel(day.date, { weekday: "long", day: "numeric", month: "long" })}: ${state === "recorded" ? duration(day.totals.focus_ms) + " focus" : state === "missing" ? "no recording" : "not yet"}`}>
        <span class="top">
          <span class="date num">{Number(day.date.slice(8))}</span>
          {#if state === "recorded"}<span class="total num">{duration(day.totals.focus_ms) || "0 min"}</span>{/if}
        </span>
        {#if state === "recorded" && day.metrics.active_ms > 0}
          <span class="bar" style:width={100 * day.metrics.active_ms / max + "%"}>
            {#each series as subject (subject.key)}
              {#if subject.days[index] > 0}<span style:flex-grow={subject.days[index]} style:background={paint.subject(subject).fill}></span>{/if}
            {/each}
          </span>
        {/if}
        <span class="meta">
          {#if state === "missing"}<span>No recording</span>
          {:else if state === "recorded" && day.metrics.longest_stretch}<span class="num">↦ {duration(day.metrics.longest_stretch.elapsed_ms)}</span>{/if}
          {#if count}<span class="commits num" title={commitCount(count)}>{count}</span>{/if}
        </span>
      </button>
    {/each}
    {#each Array(trail) as _, index (index)}<div class="blank"></div>{/each}
  </div>
  <ul class="legend" aria-label="Legend">
    {#each series as subject (subject.key)}<li><span class="dot" style:background={paint.subject(subject).fill}></span>{subject.name}</li>{/each}
  </ul>
</section>

<style>
  .month { padding: 14px; }
  .weekdays, .cells { display: grid; grid-template-columns: repeat(7, minmax(0, 1fr)); gap: 6px; }
  .weekdays { margin-bottom: 6px; }
  .weekdays span { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); padding-left: 4px; }
  .cell {
    display: flex; flex-direction: column; justify-content: space-between; gap: 8px; min-height: 96px; padding: 8px 9px;
    border-radius: 10px; border: 1px solid var(--border); background: var(--surface-2); color: var(--text); text-align: left; cursor: pointer;
    transition: border-color 0.12s, transform 0.12s;
  }
  .cell:hover { border-color: var(--border-strong); }
  .cell.today { border-color: var(--accent); box-shadow: inset 0 0 0 1px var(--accent); }
  .cell.future { background: transparent; border-style: dashed; color: var(--faint); cursor: default; }
  .cell.missing { background-color: var(--surface); }
  .blank { min-height: 96px; }
  .top { display: flex; justify-content: space-between; align-items: baseline; gap: 6px; }
  .date { font-size: 15px; font-weight: 650; }
  .today .date { color: var(--accent-strong); }
  .total { font-size: 12.5px; font-weight: 600; color: var(--text); }
  .bar { display: flex; gap: 1.5px; height: 8px; border-radius: 4px; overflow: hidden; min-width: 6px; }
  .bar span { min-width: 2px; }
  .meta { display: flex; justify-content: space-between; align-items: center; gap: 6px; font-size: 11.5px; color: var(--muted); min-height: 16px; }
  .commits { display: inline-flex; align-items: center; gap: 4px; margin-left: auto; color: var(--text-2); font-weight: 600; }
  .commits::before { content: ""; width: 7px; height: 7px; border-radius: 50%; border: 1.5px solid currentColor; }
  .legend { list-style: none; display: flex; flex-wrap: wrap; gap: 6px 14px; margin: 12px 0 0; padding: 0; font-size: 12px; color: var(--text-2); }
  .legend li { display: flex; align-items: center; gap: 6px; }
  @media (max-width: 700px) { .cell { min-height: 64px; padding: 6px; } .total, .meta span:not(.commits) { display: none; } }
</style>
