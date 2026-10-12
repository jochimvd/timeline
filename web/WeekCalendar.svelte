<script lang="ts">
  import { query } from "./api";
  import { commitCount, type CommitDay } from "./commits";
  import { dateLabel } from "./dates";
  import { clock, clockRange, duration } from "./format";
  import type { Painter } from "./paint";
  import { dayState, gapLabel } from "./trends";
  import type { DayFocusMetrics, FocusSession, Range, SessionList } from "./types";
  /** A week as calendar columns: each day's sessions placed by time of day. */
  let { days, paint, timezone, now, today, commits, onopen }: {
    days: DayFocusMetrics[]; paint: Painter; timezone: string; now: number; today: string;
    commits?: Map<string, CommitDay>; onopen: (date: string, range?: Range) => void;
  } = $props();
  const HOUR = 3_600_000;
  let sessions = $state<FocusSession[] | null>(null), error = $state("");
  let loadedFor = "";
  $effect(() => {
    const first = days[0]?.date, last = days.at(-1)?.date;
    const key = `${first}:${last}:${days.at(-1)?.evidence.observed_ms}`;
    if (!first || !last || key === loadedFor) return;
    const sameWeek = loadedFor.startsWith(`${first}:${last}:`);
    loadedFor = key;
    const controller = new AbortController();
    if (!sameWeek) sessions = null;
    query<SessionList>("sessions", { dates: { first, last } }, controller.signal)
      .then(list => { sessions = list.sessions; error = ""; })
      .catch(e => { if (!controller.signal.aborted) error = e instanceof Error ? e.message : String(e); });
    return () => controller.abort();
  });
  /** Hours after the activity day's 04:00 start, plus four: a local clock hour except across a DST change. */
  const hourOf = (day: DayFocusMetrics, ms: number) => 4 + (ms - day.range.start_ms) / HOUR;
  const byDay = $derived(days.map(day => ({ day, sessions: (sessions ?? []).filter(s => s.range.start_ms >= day.range.start_ms && s.range.start_ms < day.range.end_ms) })));
  // Show the hours with activity across the week, at least eight.
  const bounds = $derived.by(() => {
    let low = Infinity, high = -Infinity;
    for (const { day, sessions } of byDay) for (const s of sessions) {
      low = Math.min(low, hourOf(day, s.range.start_ms));
      high = Math.max(high, hourOf(day, s.range.end_ms));
    }
    if (low === Infinity) return { low: 8, high: 18 };
    low = Math.max(4, Math.floor(low)); high = Math.min(28, Math.ceil(high));
    while (high - low < 8) { if (low > 4) low--; if (high - low < 8 && high < 28) high++; }
    return { low, high };
  });
  const hours = $derived(Array.from({ length: bounds.high - bounds.low + 1 }, (_, i) => bounds.low + i));
  let height = $state(600);
  // The week fits the viewport below the header where it can.
  const pxPerHour = $derived(Math.max(36, Math.min(72, (height - 330) / (bounds.high - bounds.low))));
  const y = (day: DayFocusMetrics, ms: number) => (Math.max(bounds.low, Math.min(bounds.high, hourOf(day, ms))) - bounds.low) * pxPerHour;
  const hourLabel = (hour: number) => String(hour % 24).padStart(2, "0") + ":00";

  let hover = $state<{ session: FocusSession; x: number; y: number } | null>(null);
  function show(event: PointerEvent, session: FocusSession) {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    hover = { session, x: rect.right + 8, y: rect.top };
  }
  let viewport = $state(1200);
</script>

<svelte:window bind:innerWidth={viewport} bind:innerHeight={height} />

<section class="week card" aria-label="Week calendar">
  <div class="grid" style:--hours={bounds.high - bounds.low} style:--px={pxPerHour + "px"}>
    <div class="corner"></div>
    {#each days as day (day.date)}
      {@const state = dayState(day, now)}
      {@const count = commits?.get(day.date)?.count ?? 0}
      <button class="column-head" class:today={day.date === today} class:future={state === "future"} onclick={() => onopen(day.date)}
        aria-label={`Open ${dateLabel(day.date, { weekday: "long", day: "numeric", month: "long" })}`}>
        <span class="weekday">{dateLabel(day.date, { weekday: "short" })}</span>
        <span class="date num">{Number(day.date.slice(8))}</span>
        <span class="total num">{state === "recorded" ? duration(day.totals.focus_ms) || "0 min" : state === "missing" ? "No data" : ""}</span>
        {#if count}<span class="commit-count" title={commitCount(count)}>{count}</span>{/if}
      </button>
    {/each}

    <div class="axis num" aria-hidden="true">
      {#each hours as hour (hour)}<span style:top={(hour - bounds.low) * pxPerHour + "px"}>{hourLabel(hour)}</span>{/each}
    </div>
    {#each byDay as { day, sessions: list } (day.date)}
      {@const state = dayState(day, now)}
      <div class="column" class:today={day.date === today} class:missing={state === "missing"} class:future={state === "future"}>
        {#each hours.slice(1) as hour (hour)}<div class="hour-line" style:top={(hour - bounds.low) * pxPerHour + "px"}></div>{/each}
        {#if state === "missing"}
          <div class="missing-fill hatched"><span>No recording</span></div>
        {:else}
          {#each day.evidence.intervals as interval (interval.range.start_ms)}
            {#if interval.kind !== "observed" && interval.range.end_ms - interval.range.start_ms >= 10 * 60_000 && y(day, interval.range.end_ms) > y(day, interval.range.start_ms)}
              <div class="gap hatched" style:top={y(day, interval.range.start_ms) + "px"} style:height={y(day, interval.range.end_ms) - y(day, interval.range.start_ms) + "px"}
                title={interval.kind === "gap" ? "Not recording · " + gapLabel(interval.reason ?? "") : "No evidence"}></div>
            {/if}
          {/each}
        {/if}
        {#each list as session (session.range.start_ms)}
          {@const p = paint.session(session)}
          {@const top = y(day, session.range.start_ms)}
          {@const h = Math.max(2, y(day, session.range.end_ms) - top)}
          <button class="session" class:roomy={h >= 34} style:top={top + "px"} style:height={h + "px"}
            style:--fill={p.swatch.fill} style:--ink={p.swatch.ink}
            aria-label={`${paint.sessionTitle(session)}, ${clockRange(session.range, timezone)}, ${duration(session.elapsed_ms)}`}
            onpointerenter={event => show(event, session)} onpointerleave={() => hover = null}
            onfocus={event => hover = { session, x: event.currentTarget.getBoundingClientRect().right + 8, y: event.currentTarget.getBoundingClientRect().top }}
            onblur={() => hover = null} onclick={() => onopen(day.date, session.range)}>
            {#if h >= 18}<span class="label ellipsis">{p.label}</span>{/if}
            {#if h >= 34}<span class="length num">{duration(session.elapsed_ms)}</span>{/if}
          </button>
        {/each}
        {#each commits?.get(day.date)?.commits ?? [] as commit (commit.repository + commit.hash)}
          <span class="commit" style:top={y(day, commit.time_ms) + "px"} title={`${commit.subject} · ${clock(commit.time_ms, timezone)}`}></span>
        {/each}
        {#if day.range.start_ms <= now && now < day.range.end_ms && hourOf(day, now) >= bounds.low && hourOf(day, now) <= bounds.high}
          <div class="now" style:top={y(day, now) + "px"}></div>
        {/if}
      </div>
    {/each}
  </div>
  {#if error}<p class="error">Sessions unavailable: {error}</p>{:else if sessions === null}<p class="loading muted"><span class="spinner"></span>Loading sessions…</p>{/if}
</section>

{#if hover}
  {@const p = paint.session(hover.session)}
  <div class="tooltip" role="tooltip" style:left={Math.min(hover.x, viewport - 330) + "px"} style:top={hover.y + "px"}>
    <div class="tip-head"><span class="dot" style:background={p.swatch.fill}></span><strong>{paint.sessionTitle(hover.session)}</strong></div>
    <p class="num">{clockRange(hover.session.range, timezone)} · {duration(hover.session.elapsed_ms)}</p>
    {#each hover.session.windows as window (window.application + window.title)}
      <p class="tip-window ellipsis">{window.application} · {window.title}</p>
    {/each}
  </div>
{/if}

<style>
  .week { padding: 12px 12px 14px 0; overflow-x: auto; position: relative; }
  .grid {
    display: grid; grid-template-columns: 52px repeat(7, minmax(96px, 1fr)); grid-template-rows: auto calc(var(--hours) * var(--px));
    column-gap: 0; min-width: 760px;
  }
  .corner { border-bottom: 1px solid var(--border); }
  .column-head {
    position: relative; display: grid; grid-template-columns: auto 1fr; grid-template-rows: auto auto; column-gap: 8px; align-items: baseline;
    padding: 4px 10px 10px; border: 0; border-bottom: 1px solid var(--border); background: none; color: var(--text); text-align: left; cursor: pointer;
    border-radius: 8px 8px 0 0;
  }
  .column-head:hover { background: var(--surface-2); }
  .weekday { grid-column: 1 / -1; font-size: 11.5px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); }
  .date { font-size: 22px; font-weight: 650; letter-spacing: -0.02em; }
  .today .date { color: var(--accent-strong); }
  .total { font-size: 12px; color: var(--text-2); }
  .future .date, .future .weekday { color: var(--faint); }
  .commit-count {
    position: absolute; top: 6px; right: 8px; font-size: 10.5px; font-weight: 600; color: var(--text-2);
    display: inline-flex; align-items: center; gap: 4px;
  }
  .commit-count::before { content: ""; width: 7px; height: 7px; border-radius: 50%; border: 1.5px solid currentColor; }
  .axis { position: relative; }
  .axis span { position: absolute; right: 8px; transform: translateY(-50%); font-size: 11px; color: var(--muted); }
  .axis span:first-child { transform: none; }
  .column { position: relative; border-left: 1px solid var(--border); }
  .column.today { background: color-mix(in oklab, var(--accent), transparent 95%); }
  .hour-line { position: absolute; left: 0; right: 0; height: 1px; background: var(--border); opacity: 0.6; }
  .gap { position: absolute; left: 0; right: 0; }
  .missing-fill { position: absolute; inset: 0; display: grid; place-items: center; }
  .missing-fill span { font-size: 12px; color: var(--muted); background: var(--surface); padding: 2px 8px; border-radius: 6px; }
  .session {
    position: absolute; left: 4px; right: 6px; display: flex; flex-direction: column; justify-content: flex-start; gap: 0;
    padding: 2px 6px; border: 0; border-radius: 4px; overflow: hidden; cursor: pointer; text-align: left;
    background: color-mix(in oklab, var(--fill), var(--surface) 22%); color: var(--ink); box-shadow: 0 0 0 1px var(--surface);
  }
  .session:hover, .session:focus-visible { background: var(--fill); z-index: 2; box-shadow: 0 0 0 2px var(--text); }
  .session:not(.roomy) { justify-content: center; }
  .label { font-size: 11.5px; font-weight: 600; line-height: 1.25; }
  .length { font-size: 10.5px; opacity: 0.8; }
  .commit {
    position: absolute; right: 1px; width: 7px; height: 7px; margin-top: -3.5px; border-radius: 50%; z-index: 3;
    background: var(--surface); border: 1.5px solid var(--text);
  }
  .now { position: absolute; left: -1px; right: 0; height: 2px; background: var(--bad); z-index: 4; }
  .now::before { content: ""; position: absolute; left: -4px; top: -3px; width: 8px; height: 8px; border-radius: 50%; background: var(--bad); }
  .error { color: var(--bad); padding: 8px 16px 0; }
  .loading { display: flex; gap: 8px; align-items: center; padding: 8px 16px 0; font-size: 12.5px; }
  .tip-head { display: flex; align-items: center; gap: 6px; margin-bottom: 2px; }
  .tip-window { color: var(--muted); max-width: 300px; }
</style>
