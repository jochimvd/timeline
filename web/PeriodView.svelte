<script lang="ts">
  import ColorBy from "./ColorBy.svelte";
  import DailyFocusChart from "./DailyFocusChart.svelte";
  import HourHeatmap from "./HourHeatmap.svelte";
  import StatTile from "./StatTile.svelte";
  import SubjectComparison from "./SubjectComparison.svelte";
  import TrendChart from "./TrendChart.svelte";
  import { commitCount, commitDays, commitProblems } from "./commits";
  import { dateSpan, shortDate } from "./dates";
  import { applicationSwatch, categorySwatch, clock, clockRange, duration, swatchStyle, type Swatch } from "./format";
  import { plural } from "./metrics";
  import type { PeriodModel } from "./period";
  import { colorBy } from "./preferences";
  import { comparisonName, containsDate, periodTitle, samePoint, type Period } from "./routes";
  import { compareSubjects, durationDelta, gapReasons, heatmap, longShare, rateDelta, subjects, summarize,
    switchRate, trendSeries, type Delta, type PeriodSummary, type Subject } from "./trends";
  import type { FocusSubject, Range } from "./types";
  let { model, period, today, onperiod, onopen }: {
    model: PeriodModel; period: Period; today: string;
    /** Selects another period of the same kind; `null` for the one containing today. */
    onperiod: (date: string | null, delta?: number) => void;
    /** Opens the day view, optionally marking a range such as an hour or stretch. */
    onopen: (date: string, range?: Range) => void;
  } = $props();
  const view = $derived(model.state);
  const data = $derived($view.current);
  /** The loaded period, which may lag the requested one while it loads. */
  const shown = $derived($view.period ?? period);
  const now = $derived($view.now);
  const timezone = $derived($view.timezone);
  const configured = $derived($view.categories.map(category => category.name));
  const mode = $derived(configured.length ? $colorBy : "application");
  const noun = $derived(shown.kind);
  const name = $derived(comparisonName(shown));
  const comparison = $derived($view.comparison);
  const current = $derived(data ? summarize(data.days, now) : null);
  const previous = $derived(comparison ? summarize(comparison.data.days, now) : null);
  /** A comparison is meaningful only if the comparison period has any evidence. */
  const comparable = $derived(!!previous && previous.observed_ms > 0);
  const dates = $derived(data?.days.map(day => day.date) ?? []);
  // Commits of the loaded period only; a period still loading shows none.
  const commitList = $derived($view.commits && $view.commits.resolved.range.start_ms === data?.resolved.range.start_ms ? $view.commits : null);
  const commits = $derived(commitList ? commitDays(commitList) : undefined);
  const commitNote = $derived($view.commitError ? `Commits unavailable: ${$view.commitError}.`
    : commitProblems(commitList) ? `Commits may be missing: ${commitProblems(commitList)}.` : "");

  const swatch = (subject: Subject): Swatch => subject.category !== undefined
    ? categorySwatch($view.categories.find(category => category.name === subject.category))
    : subject.application !== undefined ? applicationSwatch(subject.application)
    : { fill: "var(--uncategorized)", ink: "var(--uncategorized-ink)" };
  const subjectSwatch = (subject: FocusSubject) => subject.kind === "category"
    ? categorySwatch($view.categories.find(category => category.name === subject.name)) : applicationSwatch(subject.name);
  const stack = $derived(data ? subjects(data.days, mode, configured) : []);
  const changes = $derived(data ? compareSubjects(data.days, comparison?.data.days ?? null, mode, configured) : []);
  const series = $derived(data ? trendSeries(data.days, mode) : null);
  // Days that have not begun have no hours to show.
  const cells = $derived(data ? heatmap(data.days.filter(day => day.range.start_ms < now), mode, timezone, now) : []);

  /** The tile's change, or why there is none. */
  function change(make: (current: PeriodSummary, previous: PeriodSummary) => Delta | undefined): Delta | string | undefined {
    if ($view.comparing && !comparison) return "Comparing…";
    if (!previous || !current) return undefined;
    if (!comparable) return `No evidence ${name}`;
    return make(current, previous);
  }
  const label = $derived(`vs ${name}`);
  const rate = (summary: PeriodSummary) => switchRate(summary, "application");
  function evidence(summary: PeriodSummary) {
    const recorded = summary.elapsed_days - summary.missing.length;
    return `${recorded} of ${plural(summary.elapsed_days, "day")} with evidence`
      + (summary.missing.length ? ` (none on ${summary.missing.map(shortDate).join(", ")})` : "");
  }
  const caveat = $derived(!!current?.missing.length || (comparable && !!previous?.missing.length));

  const niceMax = (value: number, steps: number[]) => steps.find(step => value <= step) ?? steps.at(-1)!;
  const rateMax = $derived(niceMax(Math.max(0, ...(series?.rate ?? []).map(value => value ?? 0)), [5, 10, 20, 30, 40, 60, 80, 100, 150, 200, 400]));
  const longestMax = $derived(niceMax(Math.max(0, ...(series?.longest ?? []).map(value => value ?? 0)) / 60_000, [30, 60, 90, 120, 180, 240, 360, 480, 720, 1440]) * 60_000);
  const switchTitle = $derived(`${mode === "category" ? "Category" : "Application"} switches per active hour`);
  const percent = (value: number | null) => value === null ? "—" : `${value.toFixed(0)}%`;
  const kindName = $derived(noun === "week" ? "week" : "month");
  const changeDate = (event: Event) => { const value = (event.currentTarget as HTMLInputElement).value; if (value) onperiod(value); };
</script>

<section class="panel period-panel" aria-labelledby="period-heading" aria-busy={$view.loading}>
  <nav class="timeline-navigation" aria-label="Period navigation">
    <div class="navigation-row">
      <div class="control-group date-controls">
        <button id="period-previous" aria-label={`Previous ${kindName}`} onclick={() => onperiod(period.first, -1)}>‹</button>
        <label class="date-label">{noun === "week" ? "Week of" : "Month of"}
          <input type="date" id="period-date" aria-label={`Go to the ${kindName} containing a date`} value={period.first} onchange={changeDate} />
        </label>
        <button id="period-next" aria-label={`Next ${kindName}`} onclick={() => onperiod(period.first, 1)}>›</button>
      </div>
      <button id="period-current" class="primary-action" disabled={containsDate(period, today)} onclick={() => onperiod(null)}>This {kindName}</button>
      {#if configured.length}<span class="navigation-end"><ColorBy /></span>{/if}
    </div>
  </nav>
  <div class="period-heading" class:stale={$view.loading}>
    <h2 id="period-heading">{periodTitle(shown)}</h2>
    <p class="muted">
      {noun === "month" ? dateSpan(shown.first, shown.last) + " · " : ""}Activity days begin at 04:00{#if comparison}
        {` · compared with ${dateSpan(comparison.first, comparison.last)}`}{#if comparison.partial}{` until ${clock(comparison.range.end_ms, timezone)}, ${samePoint(shown)}`}{/if}{/if}
    </p>
  </div>

  {#if data && current}
    <div class="period-content" class:stale={$view.loading}>
      <dl class="stat-tiles" id="period-totals">
        <StatTile id="period-focus" label="Recorded focus" delta={change((c, p) => durationDelta(c.focus_ms, p.focus_ms, label))}>
          {duration(current.focus_ms)}
          {#snippet detail()}{duration(current.idle_ms)} idle · {duration(current.active_ms)} active{/snippet}
        </StatTile>
        <StatTile id="period-switches" label="Context switches" delta={change((c, p) => rateDelta(rate(c), rate(p), label))}>
          {rate(current) ?? "—"}{" "}<small>per active hour</small>
          {#snippet detail()}{plural(current.switches.application, "application switch", "application switches")}{#if current.switches.category !== null}{` · ${switchRate(current, "category") ?? "—"}/h between categories`}{/if}{/snippet}
        </StatTile>
        <StatTile id="period-longest" label="Longest focus stretch" action={current.longest ? showLongest : undefined}>
          {current.longest ? duration(current.longest.stretch.elapsed_ms) : "—"}
          {#snippet detail()}
            {#if current.longest}
              <span class="category-tag"><span class="swatch" style={swatchStyle(subjectSwatch(current.longest.stretch.subject))}></span>{current.longest.stretch.subject.name}</span>
              {` · ${shortDate(current.longest.date)}, ${clockRange(current.longest.stretch.range, timezone)}`}
            {:else}No active focus{/if}
          {/snippet}
        </StatTile>
        <StatTile id="period-long" label="In long stretches" delta={change((c, p) => durationDelta(c.long_ms, p.long_ms, label))}>
          {duration(current.long_ms) || "0 min"}
          {#snippet detail()}{percent(longShare(current))} of active focus, in {plural(current.long_count, "stretch", "stretches")} of 25 min or more{/snippet}
        </StatTile>
      </dl>
      <p class="evidence-note" id="period-evidence" class:caveat>
        <span>This {kindName}{comparison?.partial || containsDate(shown, today) ? " so far" : ""}: {evidence(current)}.</span>
        {#if previous}<span>{name[0].toUpperCase() + name.slice(1)}: {comparable ? evidence(previous) : "no evidence"}.</span>{/if}
        {#if caveat && comparable}<span>Changes partly reflect missing evidence.</span>{/if}
      </p>

      <section class="period-section" aria-labelledby="daily-heading">
        <h3 id="daily-heading">Focus by day <span class="muted">· active focus by {mode}, idle on top</span></h3>
        <DailyFocusChart days={data.days} subjects={stack} {swatch} {now} {commits} onopen={date => onopen(date)}
          onshift={delta => onperiod(period.first, delta)} onhome={() => onperiod(null)} />
        {#if commitList}
          <p class="muted commit-note" id="period-commits">{commitCount(commitList.commits.length)} this {kindName} in your repositories{commitNote ? ` · ${commitNote}` : "."}</p>
        {:else if commitNote}<p class="muted commit-note" id="period-commits">{commitNote}</p>{/if}
        <details class="breakdown-details">
          <summary>Daily totals table</summary>
          <div class="table-scroll">
            <table class="data-table" id="daily-table">
              <thead><tr><th scope="col">Day</th>{#each stack as subject (subject.key)}<th scope="col">{subject.name}</th>{/each}
                <th scope="col">Idle</th><th scope="col">Recorded focus</th><th scope="col">Evidence</th><th scope="col">Gaps</th><th scope="col">Unknown</th>{#if commits}<th scope="col">Commits</th>{/if}</tr></thead>
              <tbody>
                {#each data.days as day, index (day.date)}
                  <tr><th scope="row">{shortDate(day.date)}</th>
                    {#each stack as subject (subject.key)}<td>{duration(subject.days[index])}</td>{/each}
                    <td>{duration(day.totals.focus_ms - day.metrics.active_ms)}</td><td>{duration(day.totals.focus_ms)}</td>
                    <td>{day.range.start_ms >= now ? "—" : duration(day.evidence.observed_ms)}</td>
                    <td>{duration(day.evidence.gap_ms)}{gapReasons(day).length ? ` (${gapReasons(day).join(", ")})` : ""}</td>
                    <td>{duration(day.evidence.unknown_ms)}</td>{#if commits}<td>{commits.get(day.date)?.count ?? 0}</td>{/if}</tr>
                {/each}
              </tbody>
            </table>
          </div>
        </details>
      </section>

      <div class="period-columns">
        <SubjectComparison rows={changes} {swatch} heading={mode === "category" ? "Categories" : "Applications"}
          current={`This ${kindName}`} previous={name} comparing={$view.comparing} missing={!!previous && !comparable} />
        {#if series}
          <section class="period-section trends" aria-labelledby="trends-heading">
            <h3 id="trends-heading">Focus patterns <span class="muted">· by day</span></h3>
            <TrendChart id="trend-switches" title={switchTitle} summary={`${switchRate(current, mode) ?? "—"}/h overall`} {dates}
              values={series.rate} max={rateMax} ticks={[0, rateMax / 2, rateMax]} format={value => `${Math.round(value * 10) / 10}`} onopen={date => onopen(date)} />
            <TrendChart id="trend-longest" title="Longest focus stretch" summary={current.longest ? `${duration(current.longest.stretch.elapsed_ms)} at most` : "—"} {dates}
              values={series.longest} max={longestMax} ticks={[0, longestMax / 2, longestMax]} format={duration} onopen={date => onopen(date)} />
            <TrendChart id="trend-long-share" title="Share in long stretches" summary={`${percent(longShare(current))} overall`} {dates}
              values={series.longShare} max={100} ticks={[0, 50, 100]} format={value => `${value.toFixed(0)}%`} onopen={date => onopen(date)} />
            <details class="breakdown-details">
              <summary>Focus patterns table</summary>
              <div class="table-scroll">
                <table class="data-table" id="trend-table">
                  <thead><tr><th scope="col">Day</th><th scope="col">Active focus</th><th scope="col">{switchTitle}</th>
                    <th scope="col">Longest stretch</th><th scope="col">In long stretches</th></tr></thead>
                  <tbody>
                    {#each data.days as day, index (day.date)}
                      <tr><th scope="row">{shortDate(day.date)}</th><td>{duration(day.metrics.active_ms)}</td><td>{series.rate[index] ?? "—"}</td>
                        <td>{series.longest[index] === null ? "—" : duration(series.longest[index]!)}</td><td>{percent(series.longShare[index])}</td></tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            </details>
          </section>
        {/if}
      </div>

      <section class="period-section" aria-labelledby="heatmap-heading">
        <HourHeatmap rows={cells} kind={mode} {timezone} onopen={(date, range) => onopen(date, range)} />
      </section>

      <p class="muted breakdown-note">Bars stack active focus (focus outside idle periods) by {mode}, with idle focus on top; together
        they are recorded focus. Hatched days and hours have no evidence: the recorder was paused, locked, stopped or not yet running,
        which is not the same as no activity. Changes compare the same number of activity days immediately before{#if comparison?.partial}, up
        to the same day and time{/if}. Weeks start on Monday. Context switches and focus stretches are defined as in the day view.
        This is observed activity: not time assigned to tasks, and not a measure of productivity.</p>
    </div>
  {:else if $view.loading}<p class="muted">Loading {kindName}…</p>{/if}
</section>

{#snippet showLongest()}
  <button class="link-button" onclick={() => current?.longest && onopen(current.longest.date, current.longest.stretch.range)}>Show in day view</button>
{/snippet}
