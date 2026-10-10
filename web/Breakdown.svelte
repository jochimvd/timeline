<script lang="ts">
  import { query } from "./api";
  import FocusPatterns from "./FocusPatterns.svelte";
  import StatTile from "./StatTile.svelte";
  import { byRepository, commitCount, repositoryLabel } from "./commits";
  import { applicationSwatch, captureTime, categorySwatch, clockRange, duration, swatchStyle, time, UNCATEGORIZED, type Swatch } from "./format";
  import { plural } from "./metrics";
  import type { CategoryList, CategoryRow, CommitList, Coverage, FocusMetrics, FocusSubject, Overview, Range } from "./types";
  let { rows, categories, totals, metrics, commits, range, timezone, settingsPath, loading, onreveal, onselect }: {
    rows: Coverage[]; categories: CategoryRow[]; totals?: Overview["totals"]; metrics?: FocusMetrics; commits: CommitList | null;
    range?: Range; timezone: string; settingsPath: string | null; loading: boolean;
    onreveal: (range: Range) => void;
    /** Selects a moment, such as a commit, on the timeline. */
    onselect: (ms: number) => void;
  } = $props();
  /** One ranked row: a name, optional context line, its focus share and mark color. */
  type Entry = { key: string; name: string; detail?: string; elapsed_ms: number; percent: number; swatch: Swatch };
  const ranked = (kind: string) => rows.filter(row => row.kind === kind)
    .sort((a, b) => b.elapsed_ms! - a.elapsed_ms!);
  const applications = $derived(ranked("application").map((row): Entry => ({
    key: row.application!, name: row.application!, elapsed_ms: row.elapsed_ms!,
    percent: row.percent_of_focus!, swatch: applicationSwatch(row.application!),
  })));
  const titles = $derived(ranked("exact_title").map((row): Entry => ({
    key: row.application + "\n" + row.title, name: row.title || "Untitled window",
    detail: row.application + (row.category ? " · " + row.category : ""), elapsed_ms: row.elapsed_ms!,
    percent: row.percent_of_focus!, swatch: applicationSwatch(row.application!),
  })));
  const configured = $derived(categories.some(row => row.category !== null));
  const categoryEntries = $derived(categories.map((row): Entry => ({
    key: row.category ?? "", name: row.category ?? UNCATEGORIZED, elapsed_ms: row.elapsed_ms,
    percent: row.percent_of_focus, swatch: categorySwatch(row),
  })));
  // Configured categories by time, with the uncategorized remainder last.
  const composition = $derived(categoryEntries.filter(entry => entry.elapsed_ms > 0)
    .sort((a, b) => Number(a.key === "") - Number(b.key === "")));
  const categorizedPercent = $derived(categories.filter(row => row.category !== null)
    .reduce((sum, row) => sum + row.percent_of_focus, 0));
  const subjectSwatch = (subject: FocusSubject) => subject.kind === "category"
    ? categorySwatch(categories.find(row => row.category === subject.name)) : applicationSwatch(subject.name);
  const longest = $derived(metrics?.longest_stretch ?? null);
  const commitRepositories = $derived(commits ? byRepository(commits.commits).length : 0);
  const longShare = $derived(metrics && metrics.active_ms > 0 ? 100 * metrics.stretches.long.elapsed_ms / metrics.active_ms : 0);

  // Definitions load lazily: when the rules are opened, or to explain why none apply.
  let rulesOpen = $state(false);
  let list = $state<CategoryList>();
  let listError = $state("");
  let listSignature: string | null = null;
  let listAbort: AbortController | undefined;
  const signature = $derived(categories.map(row => row.category ?? "").sort().join("\n"));
  $effect(() => {
    const current = signature;
    if ((rulesOpen || (totals && !configured)) && current !== listSignature) void loadList(current);
  });
  async function loadList(current: string) {
    listSignature = current;
    listAbort?.abort();
    const controller = listAbort = new AbortController();
    try {
      list = await query<CategoryList>("categories", {}, controller.signal);
      listError = "";
    } catch (error) {
      if (!controller.signal.aborted) { listError = error instanceof Error ? error.message : String(error); listSignature = null; }
    }
  }
  $effect(() => () => listAbort?.abort());
</script>

{#snippet showLongest()}
  <button class="link-button" onclick={() => longest && onreveal(longest.range)}>Show on timeline</button>
{/snippet}

{#snippet ranking(items: Entry[], id: string)}
  <ol {id} class="focus-ranking">
    {#each items as entry (entry.key)}
      <li class:zero={entry.elapsed_ms === 0}>
        <div class="focus-row">
          <span class="focus-name">
            <span class="app-dot" style:background={entry.swatch.fill} aria-hidden="true"></span>
            <span>{entry.name}{#if entry.detail}<small>{entry.detail}</small>{/if}</span>
          </span>
          <span class="focus-value">{entry.elapsed_ms ? duration(entry.elapsed_ms) : "—"} <span>{entry.percent.toFixed(1)}%</span></span>
        </div>
        <div class="focus-bar-track" aria-hidden="true">
          <div style:width={entry.percent + "%"} style:background={entry.swatch.fill}></div>
        </div>
      </li>
    {/each}
  </ol>
{/snippet}

<section id="breakdown" class="panel focus-breakdown" aria-labelledby="breakdown-heading" aria-busy={loading}>
  <div class="breakdown-heading">
    <h2 id="breakdown-heading">Focus summary</h2>
    <p class="muted">Loaded range{range ? " · " + captureTime(range.start_ms, timezone) + " – " + captureTime(range.end_ms, timezone) : ""}</p>
  </div>
  {#if totals && totals.focus_ms > 0}
    <dl id="totals" class="stat-tiles">
      <StatTile label="Recorded focus">
        {duration(totals.focus_ms)}
        {#snippet detail()}{duration(totals.idle_ms)} idle{#if metrics}{` · ${duration(metrics.active_ms)} active`}{/if}{/snippet}
      </StatTile>
      {#if metrics}
        <StatTile id="switch-rate" label="Context switches">
          {metrics.application_switches_per_hour ?? "—"}{" "}<small>per active hour</small>
          {#snippet detail()}{plural(metrics.application_switches, "application switch", "application switches")}{#if metrics.category_switches !== null}{` · ${metrics.category_switches_per_hour ?? "—"}/h between categories`}{/if}{/snippet}
        </StatTile>
        <StatTile id="longest-stretch" label="Longest focus stretch" action={longest ? showLongest : undefined}>
          {longest ? duration(longest.elapsed_ms) : "—"}
          {#snippet detail()}
            {#if longest}
              <span class="category-tag"><span class="swatch" style={swatchStyle(subjectSwatch(longest.subject))}></span>{longest.subject.name}</span>
              {` · ${clockRange(longest.range, timezone)}`}{#if longest.interruptions}{` · ${plural(longest.interruptions, "brief interruption")}`}{/if}
            {:else}No active focus{/if}
          {/snippet}
        </StatTile>
        <StatTile id="long-share" label="In long stretches">
          {longShare.toFixed(0)}%
          {#snippet detail()}of active focus, in {plural(metrics.stretches.long.count, "stretch", "stretches")} of 25 min or more{/snippet}
        </StatTile>
      {/if}
    </dl>
    {#if metrics && metrics.active_ms > 0}
      <FocusPatterns {metrics} {timezone} categorized={metrics.category_switches !== null} />
    {/if}
    <div class="breakdown-columns">
      <section aria-labelledby="categories-heading">
        <h3 id="categories-heading">Categories <span class="muted">· {configured ? `${categorizedPercent.toFixed(0)}% categorized` : "share of recorded focus"}</span></h3>
        {#if configured}
          <div class="composition" role="img" aria-label={"Focus by category: " + composition.map(entry => `${entry.name} ${entry.percent.toFixed(0)}%`).join(", ")}>
            {#each composition as entry (entry.key)}
              <div style:flex-grow={entry.elapsed_ms} style:background={entry.swatch.fill}
                title={`${entry.name} · ${duration(entry.elapsed_ms)} · ${entry.percent.toFixed(1)}%`}></div>
            {/each}
          </div>
          {@render ranking(categoryEntries, "categories")}
        {:else}
          <p class="empty-hint">
            {#if list?.problem}Settings were ignored: {list.problem}
            {:else}No categories configured. Add <code>[[categories]]</code> rules to
              {#if settingsPath}<code>{settingsPath}</code>{:else}the settings file{/if}
              to group focus time by project or activity.{/if}
          </p>
        {/if}
      </section>
      <section aria-labelledby="applications-heading">
        <h3 id="applications-heading">Applications <span class="muted">· {applications.length} by time</span></h3>
        {@render ranking(applications, "applications")}
      </section>
    </div>
    <details class="breakdown-details">
      <summary>Window titles <span class="muted">({titles.length})</span></summary>
      {@render ranking(titles, "titles")}
    </details>
    {#if commits?.commits.length}
      <details class="breakdown-details">
        <summary>Commits <span class="muted">({commitCount(commits.commits.length)} in {plural(commitRepositories, "repository", "repositories")})</span></summary>
        <ol class="commit-list" id="commit-list">
          {#each commits.commits as commit (commit.repository + commit.hash)}
            <li>
              <button class="link-button" aria-label={`Show ${time(commit.time_ms, timezone)} on the timeline`}
                onclick={() => onselect(commit.time_ms)}>{time(commit.time_ms, timezone)}</button>
              <span class="commit-key" aria-hidden="true"></span>
              <span class="commit-text">
                <span class="commit-subject">{commit.subject}</span>
                <small>{repositoryLabel(commit.repository)}{commit.branch ? ` · ${commit.branch}` : ""} · <code>{commit.short_hash}</code>{commit.kind === "commit" ? "" : ` · ${commit.kind.replace("_", "-")}`}{commit.amends ? ` · amended, first at ${time(commit.first_ms!, timezone)}` : ""}{commit.timing === "committer_date" ? " · committer date" : ""}</small>
              </span>
            </li>
          {/each}
        </ol>
      </details>
    {/if}
    {#if configured}
      <details class="breakdown-details" bind:open={rulesOpen}>
        <summary>Category rules <span class="muted">· first match wins</span></summary>
        {#if list}
          <ol class="category-rules" id="category-rules">
            {#each list.categories as category (category.name)}
              <li>
                <span class="category-tag"><span class="swatch" style={swatchStyle(categorySwatch(category))}></span>{category.name}</span>
                <ul>
                  {#each category.rules as rule}
                    <li>{#if rule.application}<code>{rule.application}</code>{/if}{#if rule.application && rule.title}{" with "}{/if}{#if rule.title}{"title ~ "}<code>{rule.title}</code>{/if}</li>
                  {:else}<li>No rules</li>{/each}
                </ul>
              </li>
            {/each}
          </ol>
          {#if list.problem}<p class="error">Settings were ignored: {list.problem}</p>{/if}
        {:else if listError}<p class="error">{listError}</p>{/if}
        <p class="muted">Defined in {#if settingsPath}<code>{settingsPath}</code>{:else}the settings file{/if}; edits apply to all retained history.</p>
      </details>
    {/if}
    <p class="muted breakdown-note">Recorded focus is time with a window in focus, including idle time; active focus excludes idle.
      Category and application percentages are shares of recorded focus; recording gaps are excluded.
      A context switch is a change of application or category while you were active — never across idle time, gaps or pauses.
      A focus stretch is continuous active focus on one category (or uncategorized application); glances away shorter than a minute
      don't end it while they stay under a tenth of it. This is observed activity: not time assigned to tasks, and not a measure of productivity.</p>
  {:else if totals}
    <p class="breakdown-empty">No recorded focus in this range.</p>
    <p class="muted">Choose another day or widen the timeline window to explore more activity.</p>
  {/if}
</section>
