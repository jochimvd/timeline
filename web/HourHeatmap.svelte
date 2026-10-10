<script lang="ts">
  import ChartTooltip from "./ChartTooltip.svelte";
  import { dateLabel, shortDate } from "./dates";
  import { clockRange, duration } from "./format";
  import { activeLevel, activeLevelLabel, plural, SWITCH_LEVELS, switchLevel, switchLevelLabel, type SwitchKind } from "./metrics";
  import { HEAT_HOURS, type HeatCell } from "./trends";
  import type { Range } from "./types";
  /** Hour of day × activity day on the fixed ordinal ramps of the day view, with
   *  hours without evidence hatched like recording gaps. */
  let { rows, kind, timezone, onopen }: {
    rows: { date: string; cells: (HeatCell | null)[] }[]; kind: SwitchKind; timezone: string;
    onopen: (date: string, range: Range) => void;
  } = $props();
  let metric = $state<"active" | "switches">("active");
  const switchNoun = $derived(kind === "category" ? "category switch" : "application switch");
  const step = (cell: HeatCell) => metric === "active" ? activeLevel(cell.active_ms) : switchLevel(cell.switches);
  const steps = $derived(Array.from({ length: 5 }, (_, index) => ({
    step: index + 1, label: metric === "active" ? activeLevelLabel(index + 1) : switchLevelLabel(index + 1) })));
  const compact = $derived(rows.length > 7);
  function describe(date: string, cell: HeatCell | null) {
    if (!cell) return `${shortDate(date)}: hour skipped by a clock change`;
    const when = `${shortDate(date)} ${clockRange(cell.range, timezone)}`;
    if (cell.state === "future") return `${when}: not yet begun`;
    if (cell.state === "missing") return `${when}: no evidence`;
    return `${when}: ${duration(cell.active_ms) === "0 min" ? "no" : duration(cell.active_ms)} active focus, ${plural(cell.switches, switchNoun, switchNoun + "es")}`;
  }
  let chosen = $state({ row: 0, column: 0 });
  // Kept within the rows when the period changes.
  const position = $derived({ row: Math.min(chosen.row, Math.max(0, rows.length - 1)), column: chosen.column });
  let hovered = $state<{ row: number; column: number; anchor: DOMRect } | null>(null);
  const cells: HTMLElement[] = [];
  function key(event: KeyboardEvent, row: number, column: number) {
    const moves: Record<string, [number, number]> = { ArrowLeft: [0, -1], ArrowRight: [0, 1], ArrowUp: [-1, 0], ArrowDown: [1, 0] };
    const move = moves[event.key];
    if (move) {
      event.preventDefault();
      const next = { row: Math.max(0, Math.min(rows.length - 1, row + move[0])), column: Math.max(0, Math.min(23, column + move[1])) };
      chosen = next;
      cells[next.row * 24 + next.column]?.focus();
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      open(row, column);
    } else if (event.key === "Escape") hovered = null;
  }
  function open(row: number, column: number) {
    const cell = rows[row].cells[column];
    if (cell && cell.state !== "future") onopen(rows[row].date, cell.range);
  }
  const show = (row: number, column: number, element: HTMLElement) => hovered = { row, column, anchor: element.getBoundingClientRect() };
</script>

<svelte:window onscroll={() => hovered = null} />

<div class="heatmap-heading">
  <h3 id="heatmap-heading">Hours of the day</h3>
  <fieldset class="segmented" id="heatmap-metric">
    <legend class="visually-hidden">Heatmap shows</legend>
    <div class="segmented-options">
      <label><input type="radio" name="heatmap-metric" value="active" bind:group={metric} />Active focus</label>
      <label><input type="radio" name="heatmap-metric" value="switches" bind:group={metric} />{kind === "category" ? "Category" : "Application"} switches</label>
    </div>
  </fieldset>
</div>
<ul class="legend scale-legend heatmap-legend" aria-label="Scale">
  {#each steps as entry (entry.step)}
    <li><span class="swatch" style:background={`var(--ramp-${entry.step})`}></span>{entry.label}</li>
  {/each}
  <li><span class="swatch heat-none"></span>None</li>
  <li><span class="swatch gap"></span>No evidence</li>
</ul>
<div class="heatmap" class:compact role="grid" aria-labelledby="heatmap-heading" aria-describedby={hovered ? "heatmap-tooltip" : undefined}>
  <div class="heatmap-row heatmap-hours" role="row">
    <span role="columnheader" class="heatmap-corner"><span class="visually-hidden">Day</span></span>
    {#each HEAT_HOURS as hour, column (hour)}
      <span role="columnheader" aria-label={String(hour).padStart(2, "0") + ":00"}>{column % 3 === 0 ? String(hour).padStart(2, "0") : ""}</span>
    {/each}
  </div>
  {#each rows as row, rowIndex (row.date)}
    <div class="heatmap-row" role="row">
      <span role="rowheader" class="heatmap-day">{dateLabel(row.date, { weekday: "short", day: "numeric" })}</span>
      {#each row.cells as cell, column}
        {@const level = cell && cell.state === "recorded" ? step(cell) : 0}
        <span bind:this={cells[rowIndex * 24 + column]} role="gridcell" class={"heat-cell " + (cell?.state ?? "skipped")}
          style:background={level ? `var(--ramp-${level})` : undefined}
          tabindex={rowIndex === position.row && column === position.column ? 0 : -1} aria-label={describe(row.date, cell)}
          onpointerenter={event => show(rowIndex, column, event.currentTarget)} onpointerleave={() => hovered = null}
          onfocus={event => { chosen = { row: rowIndex, column }; show(rowIndex, column, event.currentTarget); }}
          onblur={() => hovered = null} onclick={() => open(rowIndex, column)} onkeydown={event => key(event, rowIndex, column)}></span>
      {/each}
    </div>
  {/each}
</div>
<p class="muted heatmap-note">Arrow keys move between hours; Enter or a click opens the day at that hour.</p>

{#if hovered}
  {@const row = rows[hovered.row]}
  {@const cell = row.cells[hovered.column]}
  <ChartTooltip id="heatmap-tooltip" anchor={hovered.anchor}>
    <strong class="tooltip-heading">{shortDate(row.date)}{cell ? " · " + clockRange(cell.range, timezone) : ""}</strong>
    {#if !cell}<p>Skipped by a clock change.</p>
    {:else if cell.state === "future"}<p>Not yet begun.</p>
    {:else if cell.state === "missing"}<p>No evidence in this hour.</p>
    {:else}
      <table class="tooltip-values"><tbody>
        <tr><td>{duration(cell.active_ms)}</td><th scope="row">Active focus</th></tr>
        <tr><td>{cell.switches}</td><th scope="row">{cell.switches === 1 ? switchNoun : switchNoun + "es"}</th></tr>
      </tbody></table>
    {/if}
  </ChartTooltip>
{/if}
