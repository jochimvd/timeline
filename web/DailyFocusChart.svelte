<script lang="ts">
  import ChartTooltip from "./ChartTooltip.svelte";
  import { commitCount, repositorySummary, type CommitDay } from "./commits";
  import { dateLabel, shortDate, weekday } from "./dates";
  import { duration, type Swatch } from "./format";
  import { dayState, gapReasons, type Subject } from "./trends";
  import type { DayFocusMetrics } from "./types";
  /** Recorded focus per day, stacked by subject's active focus with idle on top.
   *  Days without evidence are hatched rather than drawn as zero. */
  let { days, subjects, swatch, now, commits, onopen, onshift, onhome }: {
    days: DayFocusMetrics[]; subjects: Subject[]; swatch: (subject: Subject) => Swatch; now: number;
    /** The user's commits per date, beneath the columns; absent without repositories. */
    commits?: Map<string, CommitDay>;
    onopen: (date: string) => void; onshift: (delta: number) => void; onhome: () => void;
  } = $props();
  const HOUR = 3_600_000;
  const idle = $derived(days.map(day => Math.max(0, day.totals.focus_ms - day.metrics.active_ms)));
  /** Clean hour ticks, at most four steps above zero. */
  const axis = $derived.by(() => {
    const most = Math.max(0, ...days.map(day => day.totals.focus_ms));
    const step = [1, 2, 3, 4, 6, 8].find(hours => most <= 4 * hours * HOUR) ?? 8;
    const steps = Math.max(most > 0 ? Math.ceil(most / (step * HOUR)) : 4, 1);
    return { max: steps * step * HOUR, ticks: Array.from({ length: steps + 1 }, (_, index) => index * step) };
  });
  const states = $derived(days.map(day => dayState(day, now)));
  const today = $derived(days.findIndex(day => day.range.start_ms <= now && now < day.range.end_ms));
  let width = $state(0);
  const slot = $derived(days.length ? width / days.length : 0);
  const labelEvery = $derived(slot >= 30 ? 1 : 7);
  const capLabels = $derived(slot >= 56);
  /** Counts fit beside their dot in wider columns; narrower ones show the dot. */
  const commitLabels = $derived(slot >= 30);
  const committed = (index: number) => commits?.get(days[index].date);
  let focusIndex = $state(-1);
  const tabIndex = $derived(focusIndex >= 0 && focusIndex < days.length ? focusIndex : Math.max(0, today));
  let hovered = $state<{ index: number; anchor: DOMRect } | null>(null);
  const columns: HTMLButtonElement[] = [];
  const segments = (index: number) => [
    ...subjects.map(subject => ({ key: subject.key, name: subject.name, value: subject.days[index], fill: swatch(subject).fill, idle: false })),
    { key: "idle", name: "Idle", value: idle[index], fill: "", idle: true },
  ].filter(segment => segment.value > 0);
  function describe(index: number) {
    const day = committed(index);
    return describeFocus(index) + (day?.count ? `, ${commitCount(day.count)}` : "");
  }
  function describeFocus(index: number) {
    const day = days[index], state = states[index];
    if (state === "future") return `${shortDate(day.date)}: not yet begun`;
    if (state === "missing") return `${shortDate(day.date)}: no evidence${gapReasons(day).length ? ` (${gapReasons(day).join(", ")})` : ""}`;
    return `${shortDate(day.date)}${index === today ? " so far" : ""}: ${duration(day.totals.focus_ms)} recorded focus, `
      + segments(index).map(segment => `${segment.name} ${duration(segment.value)}`).join(", ");
  }
  /** Anchors at the column's bar, or where one would stand. */
  function show(index: number, element: HTMLElement) {
    const column = element.getBoundingClientRect();
    const bar = element.querySelector(".bar-stack")?.getBoundingClientRect();
    const middle = column.left + column.width / 2;
    hovered = { index, anchor: new DOMRect(middle - 12, bar?.top ?? column.top + column.height / 2, 24, 1) };
  }
  function key(event: KeyboardEvent, index: number) {
    const move = (next: number) => { event.preventDefault(); focusIndex = next; columns[next]?.focus(); };
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      const direction = event.key === "ArrowRight" ? 1 : -1;
      if (event.shiftKey) { event.preventDefault(); onshift(direction); }
      else move(Math.max(0, Math.min(days.length - 1, index + direction)));
    } else if (event.key === "Home") { event.preventDefault(); onhome(); }
    else if (event.key === "Escape") hovered = null;
  }
</script>

<svelte:window onscroll={() => hovered = null} />

<figure class="chart daily-chart">
  <ul class="legend" aria-label="Series">
    {#each subjects as subject (subject.key)}
      <li><span class="swatch" style:background={swatch(subject).fill}></span>{subject.name}</li>
    {/each}
    <li><span class="swatch idle-swatch"></span>Idle</li>
    {#if states.includes("missing")}<li><span class="swatch gap"></span>No evidence</li>{/if}
    {#if commits}<li><span class="commit-key"></span>Your commits</li>{/if}
  </ul>
  <div class="chart-plot" style:--columns={days.length}>
    <div class="y-axis" aria-hidden="true">
      {#each axis.ticks as tick}<span style:bottom={100 * tick * HOUR / axis.max + "%"}>{tick}h</span>{/each}
    </div>
    <div class="plot-area" bind:clientWidth={width}>
      {#each axis.ticks as tick}<div class="gridline" style:bottom={100 * tick * HOUR / axis.max + "%"}></div>{/each}
      <div class="day-columns" role="group" aria-label="Days · arrows move, Enter opens the day, Shift+arrows change period">
        {#each days as day, index (day.date)}
          <button bind:this={columns[index]} class={"day-column " + states[index]} class:today={index === today}
            tabindex={index === tabIndex ? 0 : -1} aria-label={describe(index) + ". Open day."}
            aria-describedby={hovered?.index === index ? "daily-tooltip" : undefined}
            onclick={() => onopen(day.date)} onkeydown={event => key(event, index)}
            onpointerenter={event => show(index, event.currentTarget)} onpointerleave={() => hovered = null}
            onfocus={event => { focusIndex = index; show(index, event.currentTarget); }} onblur={() => hovered = null}>
            {#if states[index] === "recorded"}
              <span class="bar-stack" style:height={100 * day.totals.focus_ms / axis.max + "%"}>
                {#each segments(index) as segment (segment.key)}
                  <span class="segment" class:idle-segment={segment.idle} style:flex-grow={segment.value}
                    style:background={segment.idle ? undefined : segment.fill}></span>
                {/each}
              </span>
              {#if capLabels && day.totals.focus_ms > 0}
                <span class="cap-label" style:bottom={100 * day.totals.focus_ms / axis.max + "%"}>{duration(day.totals.focus_ms)}</span>
              {/if}
            {/if}
          </button>
        {/each}
      </div>
    </div>
    {#if commits}
      <div class="commit-row" id="commit-row" aria-hidden="true">
        {#each days as day (day.date)}
          {@const count = commits.get(day.date)?.count ?? 0}
          <span>{#if count}<span class="commit-key"></span>{#if commitLabels}{count}{/if}{/if}</span>
        {/each}
      </div>
    {/if}
    <div class="x-axis" aria-hidden="true">
      {#each days as day, index (day.date)}
        <span class:muted={states[index] === "future"} class:current={index === today}>
          {#if index % labelEvery === 0 || weekday(day.date) === 0}
            {#if days.length <= 7}{dateLabel(day.date, { weekday: "short" })}<br />{/if}{Number(day.date.slice(8))}
          {/if}
          {#if days.length <= 7 && states[index] === "missing"}<small>No evidence</small>{/if}
          {#if days.length <= 7 && index === today}<small>So far</small>{/if}
        </span>
      {/each}
    </div>
  </div>
</figure>

{#if hovered}
  {@const day = days[hovered.index]}
  {@const state = states[hovered.index]}
  <ChartTooltip id="daily-tooltip" anchor={hovered.anchor} side>
    <strong class="tooltip-heading">{shortDate(day.date)}{hovered.index === today ? " · so far" : ""}</strong>
    {#if state === "future"}<p>Not yet begun.</p>
    {:else if state === "missing"}
      <p>No evidence{gapReasons(day).length ? ` · ${gapReasons(day).join(", ")}` : " recorded"}. Not a day without activity.</p>
    {:else}
      <table class="tooltip-values">
        <tbody>
          {#each [...segments(hovered.index)].reverse() as segment (segment.key)}
            <tr><td>{duration(segment.value)}</td>
              <th scope="row"><span class="line-key" class:idle-swatch={segment.idle} style:background={segment.idle ? undefined : segment.fill}></span>{segment.name}</th></tr>
          {/each}
          <tr class="total"><td>{duration(day.totals.focus_ms)}</td><th scope="row">Recorded focus</th></tr>
        </tbody>
      </table>
      <p class="muted">Evidence {duration(day.evidence.observed_ms)}{day.evidence.gap_ms ? ` · gaps ${duration(day.evidence.gap_ms)}${gapReasons(day).length ? ` (${gapReasons(day).join(", ")})` : ""}` : ""}{day.evidence.unknown_ms ? ` · unknown ${duration(day.evidence.unknown_ms)}` : ""}</p>
    {/if}
    {#if committed(hovered.index)?.count}
      {@const day = committed(hovered.index)!}
      <p class="tooltip-commit-line"><span class="commit-key"></span>{commitCount(day.count)} · {repositorySummary(day.commits)}</p>
    {/if}
    {#if state !== "future"}<p class="muted">Click to open the day.</p>{/if}
  </ChartTooltip>
{/if}
