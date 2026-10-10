<script lang="ts">
  import ChartTooltip from "./ChartTooltip.svelte";
  import { shortDate } from "./dates";
  /** One focus metric across the period's days: a single series, so its heading
   *  names it and no legend is needed. Days without active focus break the line. */
  let { id, title, summary, dates, values, max, ticks, format, onopen }: {
    id: string; title: string; summary: string; dates: string[]; values: (number | null)[];
    max: number; ticks: number[]; format: (value: number) => string; onopen: (date: string) => void;
  } = $props();
  const HEIGHT = 96;
  let width = $state(0);
  const x = (index: number) => (index + 0.5) * width / Math.max(1, dates.length);
  const y = (value: number) => HEIGHT - HEIGHT * Math.min(value, max) / max;
  /** Consecutive days with values, each drawn as one polyline. */
  const runs = $derived.by(() => {
    const result: string[] = [];
    let run: string[] = [];
    values.forEach((value, index) => {
      if (value === null) { if (run.length) result.push(run.join(" ")); run = []; }
      else run.push(`${x(index).toFixed(1)},${y(value).toFixed(1)}`);
    });
    if (run.length) result.push(run.join(" "));
    return result;
  });
  let hovered = $state<{ index: number; anchor: DOMRect } | null>(null);
  let focused = $state(0);
  // Kept within the days when the period changes length.
  const focusIndex = $derived(Math.min(focused, Math.max(0, dates.length - 1)));
  let plot: SVGSVGElement;
  function point(index: number) {
    const bounds = plot.getBoundingClientRect(), left = bounds.left + x(index);
    const top = bounds.top + (values[index] === null ? HEIGHT : y(values[index]!));
    return new DOMRect(left - 6, top - 6, 12, 12);
  }
  function pointer(event: PointerEvent) {
    const bounds = plot.getBoundingClientRect();
    const index = Math.max(0, Math.min(dates.length - 1, Math.floor((event.clientX - bounds.left) / bounds.width * dates.length)));
    hovered = { index, anchor: point(index) };
  }
  function key(event: KeyboardEvent) {
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      focused = Math.max(0, Math.min(dates.length - 1, focusIndex + (event.key === "ArrowRight" ? 1 : -1)));
      hovered = { index: focused, anchor: point(focused) };
    } else if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onopen(dates[focusIndex]); }
    else if (event.key === "Escape") hovered = null;
  }
  const describe = (index: number) => `${shortDate(dates[index])}: ${values[index] === null ? "no active focus" : format(values[index]!)}`;
</script>

<svelte:window onscroll={() => hovered = null} />

<figure class="chart trend-chart" {id}>
  <figcaption><span>{title}</span><strong>{summary}</strong></figcaption>
  <div class="trend-plot" bind:clientWidth={width}>
    <div class="y-axis" aria-hidden="true">
      {#each ticks as tick}<span style:bottom={100 * tick / max + "%"}>{format(tick)}</span>{/each}
    </div>
    <svg bind:this={plot} width={width} height={HEIGHT} role="slider" tabindex="0"
      aria-label={title + " by day. Arrows move between days; Enter opens one."}
      aria-valuemin={0} aria-valuemax={dates.length - 1} aria-valuenow={focusIndex} aria-valuetext={describe(focusIndex)}
      aria-describedby={hovered ? id + "-tooltip" : undefined}
      onpointermove={pointer} onpointerleave={() => hovered = null} onkeydown={key}
      onfocus={() => hovered = { index: focusIndex, anchor: point(focusIndex) }} onblur={() => hovered = null}
      onclick={() => hovered && onopen(dates[hovered.index])}>
      {#each ticks as tick}<line class="gridline" x1="0" x2={width} y1={y(tick)} y2={y(tick)} />{/each}
      {#if hovered}<line class="crosshair" x1={x(hovered.index)} x2={x(hovered.index)} y1="0" y2={HEIGHT} />{/if}
      {#each runs as run}<polyline class="trend-line" points={run} />{/each}
      {#each values as value, index}
        {#if value !== null}<circle class="trend-dot" class:active={hovered?.index === index} cx={x(index)} cy={y(value)} r="4" />{/if}
      {/each}
    </svg>
  </div>
</figure>

{#if hovered}
  <ChartTooltip id={id + "-tooltip"} anchor={hovered.anchor}>
    <strong class="tooltip-heading">{shortDate(dates[hovered.index])}</strong>
    <p><span class="tooltip-value">{values[hovered.index] === null ? "—" : format(values[hovered.index]!)}</span> {title.toLowerCase()}</p>
    {#if values[hovered.index] === null}<p class="muted">No active focus.</p>{/if}
  </ChartTooltip>
{/if}
