<script lang="ts">
  import { weekday } from "./dates";
  import { activeLevel, activeLevelLabel } from "./metrics";
  import { dayState, HEAT_HOURS, heatmap } from "./trends";
  import type { DayFocusMetrics } from "./types";
  /** Average active focus by weekday and hour, over days with recorded evidence. */
  let { days, timezone, now }: { days: DayFocusMetrics[]; timezone: string; now: number } = $props();
  const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
  const grid = $derived.by(() => {
    const rows = WEEKDAYS.map(() => ({ days: 0, active: HEAT_HOURS.map(() => 0) }));
    const cells = heatmap(days.filter(day => dayState(day, now) === "recorded"), "application", timezone, now);
    for (const { date, cells: hours } of cells) {
      const row = rows[weekday(date)];
      row.days++;
      hours.forEach((cell, index) => { if (cell) row.active[index] += cell.active_ms; });
    }
    return rows.map(row => row.active.map(ms => row.days ? ms / row.days : null));
  });
  // Hours with any activity, so the grid is not mostly empty night.
  const range = $derived.by(() => {
    let low = HEAT_HOURS.length, high = -1;
    grid.forEach(row => row.forEach((ms, index) => { if (ms) { low = Math.min(low, index); high = Math.max(high, index); } }));
    return high < 0 ? { low: 5, high: 14 } : { low: Math.max(0, low - 1), high: Math.min(HEAT_HOURS.length - 1, high + 1) };
  });
  const columns = $derived(HEAT_HOURS.map((hour, index) => ({ hour, index })).slice(range.low, range.high + 1));
  let hover = $state<{ day: number; hour: number; ms: number | null; x: number; y: number } | null>(null);
  const label = (ms: number | null) => ms === null ? "No recorded days" : ms < 60_000 ? "Under a minute" : `${Math.round(ms / 60_000)} min`;
</script>

<section class="rhythm card" aria-labelledby="rhythm-heading">
  <header>
    <h2 id="rhythm-heading" class="card-title">Rhythm</h2>
    <span class="muted">Average active focus per hour, by weekday</span>
  </header>
  <div class="grid" style:--columns={columns.length} role="img" aria-label="Average active focus by weekday and hour">
    <span></span>
    {#each columns as column (column.index)}<span class="hour num">{column.hour % 3 === 0 ? String(column.hour).padStart(2, "0") : ""}</span>{/each}
    {#each grid as row, day (day)}
      <span class="weekday">{WEEKDAYS[day]}</span>
      {#each columns as column (column.index)}
        {@const ms = row[column.index]}
        <span class="cell" role="presentation" class:empty={ms === null} style:background={ms ? `var(--seq-${activeLevel(ms)})` : undefined}
          onpointerenter={event => { const r = event.currentTarget.getBoundingClientRect(); hover = { day, hour: column.hour, ms, x: r.left + r.width / 2, y: r.top }; }}
          onpointerleave={() => hover = null}></span>
      {/each}
    {/each}
  </div>
  <ul class="legend" aria-label="Scale">
    {#each [1, 2, 3, 4, 5] as step (step)}<li><span class="swatch" style:background={`var(--seq-${step})`}></span>{activeLevelLabel(step)}</li>{/each}
  </ul>
</section>

{#if hover}
  <div class="tooltip" role="tooltip" style:left={hover.x + "px"} style:top={hover.y - 8 + "px"} style:transform="translate(-50%, -100%)">
    <strong>{WEEKDAYS[hover.day]} {String(hover.hour).padStart(2, "0")}:00</strong> · {label(hover.ms)}
  </div>
{/if}

<style>
  .rhythm { padding: 14px 16px; display: flex; flex-direction: column; gap: 12px; }
  header { display: flex; align-items: baseline; gap: 10px; flex-wrap: wrap; font-size: 12.5px; }
  .grid { display: grid; grid-template-columns: 34px repeat(var(--columns), minmax(10px, 1fr)); gap: 3px; }
  .hour { font-size: 10.5px; color: var(--muted); }
  .weekday { font-size: 11.5px; color: var(--muted); align-self: center; }
  .cell { height: 20px; border-radius: 4px; background: var(--seq-0); }
  .cell.empty { background: transparent; border: 1px dashed var(--border); }
  .cell:hover { outline: 2px solid var(--text); outline-offset: 1px; }
  .legend { list-style: none; display: flex; flex-wrap: wrap; gap: 4px 14px; margin: 0; padding: 0; font-size: 11.5px; color: var(--muted); }
  .legend li { display: flex; align-items: center; gap: 6px; }
  .swatch { width: 12px; height: 12px; border-radius: 3px; }
</style>
