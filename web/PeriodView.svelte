<script lang="ts">
  import Icon from "./Icon.svelte";
  import MonthCalendar from "./MonthCalendar.svelte";
  import Patterns from "./Patterns.svelte";
  import Rhythm from "./Rhythm.svelte";
  import SubjectChanges from "./SubjectChanges.svelte";
  import WeekCalendar from "./WeekCalendar.svelte";
  import { commitDays, commitProblems } from "./commits";
  import { dateLabel, dateSpan, openPicker, shortDate } from "./dates";
  import { clock, duration } from "./format";
  import { plural } from "./metrics";
  import { painter } from "./paint";
  import type { PeriodModel } from "./period";
  import type { ColorBy } from "./preferences";
  import { comparisonName, containsDate, samePoint, type Period } from "./routes";
  import { durationDelta, longShare, rateDelta, summarize, switchRate, type Delta, type PeriodSummary } from "./trends";
  import type { Range } from "./types";
  let { model, period, today, choice, onperiod, onopen }: {
    model: PeriodModel; period: Period; today: string; choice: ColorBy;
    /** Selects another period of the same kind; `null` for the one containing today. */
    onperiod: (date: string | null, delta?: number) => void;
    /** Opens the day view, optionally marking a range such as a session. */
    onopen: (date: string, range?: Range) => void;
  } = $props();
  const view = $derived(model.state);
  const data = $derived($view.current);
  /** The loaded period, which may lag the requested one while it loads. */
  const shown = $derived($view.period ?? period);
  const now = $derived($view.now);
  const timezone = $derived($view.timezone);
  const paint = $derived(painter(choice, $view.categories));
  const comparison = $derived($view.comparison);
  const current = $derived(data ? summarize(data.days, now) : null);
  const previous = $derived(comparison ? summarize(comparison.data.days, now) : null);
  const comparable = $derived(!!previous && previous.observed_ms > 0);
  const name = $derived(comparisonName(shown));
  const kind = $derived(shown.kind);
  const inProgress = $derived(containsDate(shown, today));
  const commitList = $derived($view.commits && $view.commits.resolved.range.start_ms === data?.resolved.range.start_ms ? $view.commits : null);
  const commits = $derived(commitList ? commitDays(commitList) : undefined);
  const commitNote = $derived($view.commitError ? `Commits unavailable: ${$view.commitError}` : commitProblems(commitList));

  function change(make: (c: PeriodSummary, p: PeriodSummary) => Delta | undefined): Delta | string | undefined {
    if ($view.comparing && !comparison) return "…";
    if (!previous || !current) return undefined;
    if (!comparable) return "no comparison";
    return make(current, previous);
  }
  const label = $derived(`vs ${name}`);
  const deep = (s: PeriodSummary) => longShare(s);
  const deepDelta = (c: PeriodSummary, p: PeriodSummary): Delta | undefined => {
    const a = deep(c), b = deep(p);
    if (a === null || b === null) return undefined;
    const difference = Math.round(a) - Math.round(b);
    return { text: difference === 0 ? "No change" : `${difference > 0 ? "+" : "−"}${Math.abs(difference)} pts`, direction: Math.sign(difference) as Delta["direction"], label };
  };
  const tiles = $derived(current ? [
    { id: "period-focus", label: "Focus", value: duration(current.focus_ms) || "0 min",
      caption: `${duration(current.active_ms) || "0 min"} active`, delta: change((c, p) => durationDelta(c.focus_ms, p.focus_ms, label)) },
    { id: "period-switches", label: "Switches", value: String(switchRate(current, paint.mode) ?? "—"), unit: "/h",
      caption: paint.mode === "category" ? "between categories" : "between apps", delta: change((c, p) => rateDelta(switchRate(c, paint.mode), switchRate(p, paint.mode), label)) },
    { id: "period-deep", label: "Deep focus", value: deep(current) === null ? "—" : deep(current)!.toFixed(0), unit: "%",
      caption: `${duration(current.long_ms) || "0 min"} in stretches ≥ 25 min`, delta: change(deepDelta) },
  ] : []);
  const evidenceNote = $derived(current?.missing.length ? `No recording on ${current.missing.map(shortDate).join(", ")}` : "");
  const changeDate = (event: Event) => { const value = (event.currentTarget as HTMLInputElement).value; if (value) onperiod(value); };
  const weekNumber = (date: string) => {
    const d = new Date(date + "T12:00:00Z");
    d.setUTCDate(d.getUTCDate() + 3 - (d.getUTCDay() + 6) % 7);
    const first = new Date(Date.UTC(d.getUTCFullYear(), 0, 4));
    return 1 + Math.round(((d.getTime() - first.getTime()) / 86_400_000 - 3 + (first.getUTCDay() + 6) % 7) / 7);
  };
  const heading = $derived(kind === "week" ? `Week ${weekNumber(shown.first)}` : dateLabel(shown.first, { month: "long", year: "numeric" }));
  const subheading = $derived(kind === "week" ? dateSpan(shown.first, shown.last) : `${shown.length} days`);
</script>

{#snippet deltaBadge(delta: Delta | string | undefined)}
  {#if typeof delta === "string"}<span class="delta none">{delta}</span>
  {:else if delta}<span class="delta" class:up={delta.direction > 0} class:down={delta.direction < 0} title={delta.label}>
    {delta.direction > 0 ? "↑" : delta.direction < 0 ? "↓" : ""} {delta.text.replace(/^[+−]/, "")}</span>{/if}
{/snippet}

<div class="period" aria-busy={$view.loading}>
  <header class="head">
    <div class="title-block">
      <div class="date-nav">
        <button id="period-previous" class="btn icon ghost" aria-label={`Previous ${kind}`} onclick={() => onperiod(period.first, -1)}><Icon name="chevron-left" /></button>
        <label class="date-picker">
          <h1 id="period-heading">{heading}</h1>
          <Icon name="chevron-down" size={14} />
          <input type="date" id="period-date" onclick={openPicker} aria-label={`Go to the ${kind} containing a date`} value={period.first} onchange={changeDate} />
        </label>
        <button id="period-next" class="btn icon ghost" aria-label={`Next ${kind}`} onclick={() => onperiod(period.first, 1)}><Icon name="chevron-right" /></button>
        {#if !containsDate(period, today)}<button id="period-current" class="btn small" onclick={() => onperiod(null)}>This {kind}</button>{/if}
      </div>
      <p class="subtitle muted">{subheading}{#if comparison}{` · compared with ${dateSpan(comparison.first, comparison.last)}`}{#if comparison.partial}{` until ${clock(comparison.range.end_ms, timezone)}`}{/if}{/if}</p>
    </div>
    <dl class="kpis" id="period-totals" class:loading={$view.loading}>
      {#each tiles as tile (tile.id)}
        <div class="kpi" id={tile.id}>
          <dt>{tile.label}</dt>
          <dd class="num">{tile.value}{#if tile.unit}<small>{tile.unit}</small>{/if}{@render deltaBadge(tile.delta)}</dd>
          <span class="caption ellipsis">{tile.caption}</span>
        </div>
      {/each}
      {#if current}
        <div class="kpi" id="period-longest">
          <dt>Longest stretch</dt>
          <dd class="num">{current.longest ? duration(current.longest.stretch.elapsed_ms) : "—"}</dd>
          {#if current.longest}
            {@const longest = current.longest}
            <button class="caption link" onclick={() => onopen(longest.date, longest.stretch.range)}>
              {longest.stretch.subject.kind === "category" ? paint.category(longest.stretch.subject.name).label : longest.stretch.subject.name} · {shortDate(longest.date)}</button>
          {:else}<span class="caption">&nbsp;</span>{/if}
        </div>
        {#if commitList}
          <div class="kpi" id="period-commits">
            <dt>Commits</dt>
            <dd class="num">{commitList.commits.length}</dd>
            <span class="caption">{plural(commitList.repositories, "repository", "repositories")}</span>
          </div>
        {/if}
      {/if}
    </dl>
  </header>

  {#if evidenceNote || commitNote || (comparison && comparable && previous?.missing.length)}
    <p class="note" id="period-evidence">
      <Icon name="info" size={14} />
      <span>{[evidenceNote, comparable && previous?.missing.length ? `${name[0].toUpperCase() + name.slice(1)} has days without recording, so changes partly reflect missing data` : "", commitNote].filter(Boolean).join(" · ")}</span>
    </p>
  {/if}

  {#if data && current}
    <div class="layout" class:stale={$view.loading}>
      <div class="main">
        {#if kind === "week"}
          <WeekCalendar days={data.days} {paint} {timezone} {now} {today} {commits} {onopen} />
        {:else}
          <MonthCalendar days={data.days} {paint} {now} {today} {commits} {onopen} />
          <Rhythm days={data.days} {timezone} {now} />
        {/if}
      </div>
      <aside class="side">
        <SubjectChanges days={data.days} previous={comparable ? comparison!.data.days : null} {paint} comparing={$view.comparing}
          current={inProgress ? `This ${kind} so far` : `This ${kind}`} {name} />
        <Patterns days={data.days} {paint} {now} {onopen} />
      </aside>
    </div>
  {:else if $view.loading}
    <div class="loading"><span class="spinner"></span>Loading {kind}…</div>
  {/if}
  <p class="foot muted">Changes compare with the {name}{inProgress ? `, up to ${samePoint(shown)}` : ""}. Ticks mark the {name}; arrows are changes, not judgements.</p>
</div>

<style>
  .period { display: flex; flex-direction: column; gap: 16px; }
  .head { display: flex; align-items: flex-end; justify-content: space-between; gap: 24px; flex-wrap: wrap; }
  .title-block { display: flex; flex-direction: column; gap: 2px; }
  .date-nav { display: flex; align-items: center; gap: 4px; margin-left: -8px; }
  .date-picker { position: relative; display: flex; align-items: center; gap: 6px; padding: 2px 8px; border-radius: 8px; cursor: pointer; color: var(--muted); }
  .date-picker:hover { background: var(--surface-2); }
  .date-picker:focus-within { outline: 2px solid var(--accent); }
  .date-picker h1 { font-size: 24px; font-weight: 650; letter-spacing: -0.02em; color: var(--text); white-space: nowrap; }
  .date-picker input { position: absolute; inset: 0; opacity: 0; cursor: pointer; width: 100%; }
  .date-picker input::-webkit-calendar-picker-indicator { position: absolute; inset: 0; width: 100%; height: 100%; cursor: pointer; }
  .subtitle { font-size: 13px; padding-left: 2px; }
  .kpis { display: flex; gap: 8px; flex-wrap: wrap; }
  .kpi { display: flex; flex-direction: column; min-width: 140px; padding: 10px 14px; border-radius: var(--radius); background: var(--surface); border: 1px solid var(--border); }
  .kpis.loading .kpi { opacity: 0.6; }
  dt { font-size: 11.5px; color: var(--muted); font-weight: 500; }
  dd { font-size: 22px; font-weight: 650; letter-spacing: -0.02em; line-height: 1.25; }
  dd small { font-size: 13px; color: var(--muted); font-weight: 500; margin-left: 1px; }
  .caption { display: block; font-size: 11.5px; color: var(--muted); white-space: nowrap; max-width: 220px; text-align: left; }
  .caption.link { color: var(--accent-strong); overflow: hidden; text-overflow: ellipsis; display: block; }
  .delta { margin-left: 8px; font-size: 12px; font-weight: 600; letter-spacing: 0; color: var(--text-2); vertical-align: 2px; }
  .delta.none { font-weight: 500; color: var(--faint); }
  .note { display: flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--text-2); padding: 8px 12px; border-radius: var(--radius-sm); background: var(--surface); border: 1px solid var(--border); }
  .layout { display: grid; grid-template-columns: minmax(0, 1fr) 360px; gap: 16px; align-items: start; transition: opacity 0.2s; }
  .layout.stale { opacity: 0.6; }
  .main, .side { display: flex; flex-direction: column; gap: 16px; min-width: 0; }
  .loading { display: flex; align-items: center; gap: 10px; color: var(--muted); padding: 40px; justify-content: center; }
  .foot { font-size: 12px; }
  @media (max-width: 1180px) { .layout { grid-template-columns: 1fr; } }
  @media (max-width: 700px) { .kpi { min-width: calc(50% - 4px); flex: 1; } .date-picker h1 { font-size: 19px; } }
</style>
