<script lang="ts">
  import CommitList from "./CommitList.svelte";
  import DatePicker from "./DatePicker.svelte";
  import Icon from "./Icon.svelte";
  import Journal from "./Journal.svelte";
  import Notice, { type Note } from "./Notice.svelte";
  import Scrubber from "./Scrubber.svelte";
  import Stage from "./Stage.svelte";
  import Summary from "./Summary.svelte";
  import type { CategoryNode } from "./categories";
  import { commitProblems } from "./commits";
  import { addDays, dateLabel } from "./dates";
  import { drawer } from "./drawer";
  import { clock, clockRange, duration } from "./format";
  import { plural } from "./metrics";
  import type { TimelineApp } from "./model";
  import { painter } from "./paint";
  import type { ColorBy } from "./preferences";
  import { longShare, switchRate } from "./trends";
  import type { Range } from "./types";
  let { app, nodes, choice, today }: { app: TimelineApp; nodes: CategoryNode[]; choice: ColorBy; today: string } = $props();
  const view = $derived(app.state);
  const detail = $derived(app.details);
  // Breakdown rows carry each category's color; configured categories fill in before they load.
  const known = $derived($view.categories.length ? $view.categories.flatMap(row => row.category === null ? []
    : [{ name: row.category, parent: row.parent, color: row.color, palette: row.palette }]) : nodes);
  const paint = $derived(painter(choice, known));
  // With nothing selected yet, opening a day selects its first session.
  let framed = "";
  $effect(() => {
    const state = $view, list = $detail.sessions;
    if ($detail.date !== state.shown || !list || state.dayLoading || framed === state.shown) return;
    framed = state.shown;
    if (!list.length) return;
    // A moment into the session, past its first capture.
    const first = list[0].range;
    if (state.frames === null && !state.evidenceLoading) void app.select(Math.min(first.end_ms - 1, first.start_ms + 15_000));
  });

  /** The day on screen, which stays while another loads. */
  const shown = $derived($view.shown);
  const day = $derived($detail.date === shown ? $detail.day : null);
  const metrics = $derived(day?.metrics);
  const switches = $derived(metrics ? switchRate({ switches: { application: metrics.application_switches, category: metrics.category_switches },
    active_ms: metrics.active_ms }, paint.mode) : null);
  const deep = $derived(metrics ? longShare({ long_ms: metrics.stretches.long.elapsed_ms, active_ms: metrics.active_ms }) : null);
  const longest = $derived(metrics?.longest_stretch ?? null);
  const sessions = $derived($detail.date === shown ? $detail.sessions : null);
  const span = $derived(sessions?.length ? { start_ms: sessions[0].range.start_ms, end_ms: sessions.at(-1)!.range.end_ms } : null);
  const whole = $derived(!!$view.day && !!$view.loadedRange && $view.loadedRange.start_ms <= $view.day.start_ms && $view.loadedRange.end_ms >= $view.day.end_ms);

  const repositories = { label: "Repositories", run: () => drawer.set("repositories") };
  const notes = $derived.by(() => {
    const list: Note[] = [];
    if ($detail.error && $detail.date === shown) list.push({ label: "Totals unavailable", text: `Day totals and sessions unavailable: ${$detail.error}`, warn: true });
    if ($view.commitError) list.push({ label: "Commits unavailable", text: `Commits unavailable: ${$view.commitError}`, warn: true, action: repositories });
    else if (commitProblems($view.commits)) list.push({ label: "Commits incomplete", text: commitProblems($view.commits), warn: true, action: repositories });
    return list;
  });

  let tab = $state<"sessions" | "summary" | "commits">("sessions");
  const commitCount = $derived($view.commits?.configured ? $view.commits.commits.length : null);

  function reveal(range: Range) {
    document.getElementById("timeline")?.focus({ preventScroll: true });
    void app.reveal(range);
  }
  function select(ms: number) {
    document.getElementById("timeline")?.focus({ preventScroll: true });
    void app.navigateTo(ms);
  }
  /** Shortcuts anywhere on the page, except while typing or in a dialog. */
  function key(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey || document.querySelector("dialog[open]")
      || target.closest("input, select, textarea, [contenteditable], [role=slider], [role=dialog]")) return;
    const state = $view;
    if (!state.visible || state.dayLoading) return;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      const direction = event.key === "ArrowRight" ? 1 : -1;
      if (event.shiftKey) app.pan(direction); else void app.stepScreenshot(direction);
    } else if (event.key === "," || event.key === ".") { event.preventDefault(); void app.stepScreenshot(event.key === "." ? 1 : -1, true); }
    else if (event.key === "+" || event.key === "=" || event.key === "-") { event.preventDefault(); app.stepZoom(event.key === "-" ? -1 : 1); }
    else if (event.key === "[" || event.key === "]") { event.preventDefault(); app.shiftDate(event.key === "]" ? 1 : -1); }
    else if (event.key === "Home") { event.preventDefault(); void app.jumpNow(); }
  }
  const title = $derived(shown ? dateLabel(shown, { weekday: "long", day: "numeric", month: "long" }) : "");
  const year = $derived(shown && shown.slice(0, 4) !== today.slice(0, 4) ? shown.slice(0, 4) : "");
</script>

<svelte:window onkeydown={key} />

<div class="day" aria-busy={$view.dayLoading}>
  <header class="head">
    <div class="title-block">
      <div class="date-nav">
        <button id="previous" class="btn icon ghost" aria-label="Previous day" title="Previous day ([)" disabled={!$view.day} onclick={() => app.shiftDate(-1)}>
          <Icon name="chevron-left" /></button>
        <DatePicker kind="day" value={shown} {today} max={addDays(today, 1)} id="date" title="Choose an activity day" onselect={date => void app.loadDay(date)}>
          <span>{title}{#if year}<span class="year"> {year}</span>{/if}</span>
        </DatePicker>
        <button id="next" class="btn icon ghost" aria-label="Next day" title="Next day (])" disabled={!$view.day} onclick={() => app.shiftDate(1)}>
          <Icon name="chevron-right" /></button>
        {#if shown && shown !== today}
          <button id="today" class="btn small" onclick={() => void app.openToday()}>Today</button>
        {/if}
      </div>
      <div class="subtitle muted num">
        <span>{#if span}{clockRange(span, $view.timezone)} · {plural(sessions!.length, "session")}{:else if sessions}No active focus{:else}&nbsp;{/if}
          {#if shown === today}<span class="live">· live</span>{/if}</span>
        <Notice id="day-notes" {notes} />
      </div>
    </div>
    <dl class="kpis" id="day-totals" class:loading={!day || $view.dayLoading}>
      <div class="kpi">
        <dt>Focus</dt>
        <dd class="num">{day ? duration(day.totals.focus_ms) || "0 min" : "—"}</dd>
        <span class="caption num">{day ? `${duration(day.metrics.active_ms) || "0 min"} active · ${duration(day.totals.idle_ms) || "0 min"} idle` : " "}</span>
      </div>
      <div class="kpi">
        <dt>Switches</dt>
        <dd class="num">{switches ?? "—"}<small>/h</small></dd>
        <span class="caption">{paint.mode === "category" ? "between categories" : "between apps"}</span>
      </div>
      <div class="kpi">
        <dt>Longest stretch</dt>
        <dd class="num">{longest ? duration(longest.elapsed_ms) : "—"}</dd>
        {#if longest}
          <button class="caption link" onclick={() => reveal(longest.range)} title="Show on the timeline">
            {longest.subject.kind === "category" ? paint.category(longest.subject.name).label : longest.subject.name} · {clock(longest.range.start_ms, $view.timezone)}
          </button>
        {:else}<span class="caption">&nbsp;</span>{/if}
      </div>
      <div class="kpi">
        <dt>Deep focus</dt>
        <dd class="num">{deep === null ? "—" : deep.toFixed(0)}<small>%</small></dd>
        <span class="caption">{metrics ? `in ${plural(metrics.stretches.long.count, "stretch", "stretches")} ≥ 25 min` : " "}</span>
      </div>
      {#if commitCount !== null}
        <div class="kpi">
          <dt>Commits</dt>
          <dd class="num">{commitCount}</dd>
          <span class="caption">{whole ? "this day" : "in view"}</span>
        </div>
      {/if}
    </dl>
  </header>

  <div class="layout">
    <div class="main">
      <Stage view={$view} {app} {paint} />
      <Scrubber view={$view} {app} {paint} {sessions} evidence={day?.evidence ?? null} />
      <p class="hints muted"><kbd>←</kbd><kbd>→</kbd> next change · <kbd>,</kbd><kbd>.</kbd> one screenshot · <kbd>Shift</kbd>+<kbd>←</kbd><kbd>→</kbd> pan · <kbd>+</kbd><kbd>−</kbd> zoom ·
        <kbd>[</kbd><kbd>]</kbd> day · <kbd>Home</kbd> now · drag to pan, scroll to zoom</p>
    </div>
    <aside class="panel card">
      <div class="tabs segmented" role="tablist" aria-label="Day details">
        <button role="tab" aria-selected={tab === "sessions"} onclick={() => tab = "sessions"}><Icon name="list" size={14} />Sessions</button>
        <button role="tab" aria-selected={tab === "summary"} onclick={() => tab = "summary"}><Icon name="pie" size={14} />Breakdown</button>
        <button role="tab" aria-selected={tab === "commits"} onclick={() => tab = "commits"}><Icon name="commit" size={14} />Commits{#if commitCount}<span class="count">{commitCount}</span>{/if}</button>
      </div>
      <div class="panel-body" class:stale={$view.dayLoading} role="tabpanel">
        {#if tab === "sessions"}
          <Journal {sessions} evidence={day?.evidence ?? null} {paint} timezone={$view.timezone} selected={$view.selected}
            loading={$detail.loading || $view.dayLoading} error={$detail.error} onreveal={reveal} />
        {:else if tab === "summary"}
          <Summary rows={$view.rows} categories={$view.categories} {paint} range={$view.loadedRange} {whole}
            timezone={$view.timezone} loading={$view.rangeLoading} />
        {:else}
          <CommitList commits={$view.commits} error={$view.commitError} timezone={$view.timezone} onselect={select}
            onrepositories={() => drawer.set("repositories")} />
        {/if}
      </div>
    </aside>
  </div>
</div>

<style>
  /* The day fills the viewport below the top bar; only its panels scroll. */
  .day { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 16px; }
  .head { display: flex; align-items: flex-end; justify-content: space-between; gap: 24px; flex-wrap: wrap; }
  .title-block { display: flex; flex-direction: column; gap: 2px; }
  .date-nav { display: flex; align-items: center; gap: 4px; margin-left: -8px; }
  .year { color: var(--muted); font-weight: 500; }
  /* Tall enough for a notice, so one appearing moves nothing. */
  .subtitle { display: flex; flex-wrap: wrap; align-items: center; gap: 2px 8px; min-height: 22px; font-size: 13px; padding-left: 2px; }
  .live { color: var(--good); }
  .kpis { display: flex; gap: 8px; flex-wrap: wrap; }
  .kpi {
    display: flex; flex-direction: column; min-width: 128px; padding: 10px 14px; border-radius: var(--radius);
    background: var(--surface); border: 1px solid var(--border);
  }
  .kpis.loading .kpi { opacity: 0.6; }
  dt { font-size: 11.5px; color: var(--muted); font-weight: 500; }
  dd { font-size: 22px; font-weight: 650; letter-spacing: -0.02em; line-height: 1.25; }
  dd small { font-size: 13px; color: var(--muted); font-weight: 500; margin-left: 1px; }
  .caption { font-size: 11.5px; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 180px; text-align: left; }
  .caption.link { color: var(--accent-strong); }
  .layout {
    flex: 1; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr) 380px; grid-template-rows: minmax(0, 1fr); gap: 16px;
  }
  /* Stage and timeline share the height; screenshots take what the timeline leaves. */
  .main { display: flex; flex-direction: column; gap: 12px; min-width: 0; min-height: 0; }
  .main > :global(.stage) { flex: 1; }
  .hints { font-size: 11.5px; display: flex; flex-wrap: wrap; align-items: center; gap: 3px; }
  kbd {
    display: inline-grid; place-items: center; min-width: 18px; height: 18px; padding: 0 4px; border-radius: 4px;
    font: 600 10.5px/1 inherit; background: var(--surface-2); border: 1px solid var(--border-strong); color: var(--text-2);
  }
  .panel { display: flex; flex-direction: column; min-height: 0; overflow: hidden; }
  .tabs { margin: 10px; align-self: stretch; }
  .tabs button { flex: 1; justify-content: center; }
  .count { font-size: 11px; padding: 0 5px; border-radius: 4px; background: var(--surface-3); color: var(--text-2); }
  .panel-body { flex: 1; overflow: auto; padding: 4px 10px 14px; }
  .panel-body.stale { opacity: 0.6; }
  @media (max-width: 1180px) {
    .layout { grid-template-columns: 1fr; grid-template-rows: auto; }
    .main { height: max(560px, calc(100vh - 196px)); }
    .panel { max-height: 80vh; }
  }
  @media (max-width: 700px) {
    .main { height: auto; }
    .main :global(.stage .viewport) { flex: none; }
    .main :global(.stage .shot canvas) { max-height: 60vh; }
    .hints { display: none; }
    .kpi { min-width: calc(50% - 4px); flex: 1; }
    }
</style>
