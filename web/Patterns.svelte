<script lang="ts">
  import { shortDate } from "./dates";
  import { duration } from "./format";
  import type { Painter } from "./paint";
  import { dayState, trendSeries } from "./trends";
  import type { DayFocusMetrics, Range } from "./types";
  /** Per-day focus patterns as small multiples on independent scales. */
  let { days, paint, now, onopen }: {
    days: DayFocusMetrics[]; paint: Painter; now: number; onopen: (date: string, range?: Range) => void;
  } = $props();
  const series = $derived(trendSeries(days, paint.mode));
  const active = $derived(days.map(day => dayState(day, now) === "recorded" ? day.metrics.active_ms : null));
  const nice = (value: number, steps: number[]) => steps.find(step => value <= step) ?? steps.at(-1)!;
  const mean = (values: (number | null)[]) => { const v = values.filter((x): x is number => x !== null && x > 0); return v.length ? v.reduce((a, b) => a + b, 0) / v.length : null; };
  const most = (values: (number | null)[]) => { const v = values.filter((x): x is number => x !== null); return v.length ? Math.max(...v) : null; };
  const charts = $derived([
    { id: "trend-active", title: "Active focus", values: active, max: nice(Math.max(0, ...active.map(v => v ?? 0)) / 3_600_000, [2, 4, 6, 8, 10, 12, 16, 24]) * 3_600_000,
      format: (v: number) => duration(v) || "0 min", bars: true, summary: "avg", overall: mean(active) },
    { id: "trend-switches", title: `Switches per hour`, values: series.rate, max: nice(Math.max(0, ...series.rate.map(v => v ?? 0)), [5, 10, 20, 30, 40, 60, 100, 200]),
      format: (v: number) => `${Math.round(v * 10) / 10}/h`, bars: false, summary: "avg", overall: mean(series.rate) },
    { id: "trend-longest", title: "Longest stretch", values: series.longest, max: nice(Math.max(0, ...series.longest.map(v => v ?? 0)) / 60_000, [30, 60, 90, 120, 180, 240, 480]) * 60_000,
      format: (v: number) => duration(v), bars: false, summary: "max", overall: most(series.longest) },
    { id: "trend-deep", title: "Deep focus", values: series.longShare, max: 100, format: (v: number) => `${v.toFixed(0)}%`, bars: false, summary: "avg", overall: mean(series.longShare) },
  ]);
  const W = 300, H = 56;
  const x = (index: number) => days.length <= 1 ? W / 2 : 6 + index * (W - 12) / (days.length - 1);
  const yOf = (value: number, max: number) => H - 4 - (H - 10) * Math.min(1, value / Math.max(max, 1e-9));
  function path(values: (number | null)[], max: number) {
    let d = "", pen = false;
    values.forEach((value, index) => {
      if (value === null) { pen = false; return; }
      d += `${pen ? "L" : "M"}${x(index).toFixed(1)},${yOf(value, max).toFixed(1)}`;
      pen = true;
    });
    return d;
  }
  let hover = $state<number | null>(null);
  function pointer(event: PointerEvent) {
    const rect = (event.currentTarget as SVGElement).getBoundingClientRect();
    const f = (event.clientX - rect.left) / rect.width * W;
    hover = Math.max(0, Math.min(days.length - 1, Math.round((f - 6) / ((W - 12) / Math.max(1, days.length - 1)))));
  }
</script>

<section class="patterns card" aria-labelledby="patterns-heading">
  <header>
    <h2 id="patterns-heading" class="card-title">Focus patterns</h2>
    <span class="muted num">{hover === null ? "by day" : shortDate(days[hover].date)}</span>
  </header>
  {#each charts as chart (chart.id)}
    {@const value = hover === null ? null : chart.values[hover]}
    <figure id={chart.id}>
      <figcaption><span>{chart.title}</span><span class="value num">{#if hover !== null}{value === null ? "—" : chart.format(value)}{:else if chart.overall !== null}<span class="muted">{chart.summary}</span> {chart.format(chart.overall)}{/if}</span></figcaption>
      <!-- Days open from the calendar; the chart's click is a pointer shortcut. -->
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" role="img" aria-label={`${chart.title} by day`}
        onpointermove={pointer} onpointerleave={() => hover = null} onclick={() => hover !== null && onopen(days[hover].date)}>
        <line x1="0" x2={W} y1={H - 4} y2={H - 4} class="base" />
        {#if chart.bars}
          {#each chart.values as v, index (index)}
            {#if v}<rect x={x(index) - Math.max(2, (W - 12) / days.length * 0.32)} width={Math.max(4, (W - 12) / days.length * 0.64)}
              y={yOf(v, chart.max)} height={H - 4 - yOf(v, chart.max)} rx="2" class="bar" class:on={hover === index} />{/if}
          {/each}
        {:else}
          <path d={path(chart.values, chart.max)} class="line" vector-effect="non-scaling-stroke" />
          {#each chart.values as v, index (index)}
            {#if v !== null && (hover === index || days.length <= 10)}<circle cx={x(index)} cy={yOf(v, chart.max)} r={hover === index ? 3.5 : 2.2} class="point" />{/if}
          {/each}
        {/if}
        {#if hover !== null}<line x1={x(hover)} x2={x(hover)} y1="0" y2={H - 4} class="cross" vector-effect="non-scaling-stroke" />{/if}
      </svg>
    </figure>
  {/each}
</section>

<style>
  .patterns { padding: 14px 16px; display: flex; flex-direction: column; gap: 12px; }
  header { display: flex; align-items: baseline; justify-content: space-between; font-size: 12px; }
  figure { display: flex; flex-direction: column; gap: 4px; }
  figcaption { display: flex; justify-content: space-between; font-size: 12px; color: var(--muted); }
  .value { color: var(--text); font-weight: 600; }
  .value .muted { font-weight: 500; }
  svg { width: 100%; height: 56px; cursor: pointer; overflow: visible; }
  .base { stroke: var(--border-strong); stroke-width: 1; }
  .line { fill: none; stroke: var(--seq-4); stroke-width: 2; stroke-linejoin: round; }
  .point { fill: var(--seq-4); stroke: var(--surface); stroke-width: 1.5; }
  .bar { fill: color-mix(in oklab, var(--seq-3), var(--surface) 25%); }
  .bar.on { fill: var(--seq-4); }
  .cross { stroke: var(--text-2); stroke-width: 1; stroke-dasharray: 2 2; }
</style>
