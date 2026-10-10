<script lang="ts">
  import { query } from "./api";
  import { applicationSwatch, captureTime, categorySwatch, duration, swatchStyle, UNCATEGORIZED, type Swatch } from "./format";
  import type { CategoryList, CategoryRow, Coverage, Overview, Range } from "./types";
  let { rows, categories, totals, range, timezone, settingsPath, loading }: {
    rows: Coverage[]; categories: CategoryRow[]; totals?: Overview["totals"]; range?: Range;
    timezone: string; settingsPath: string | null; loading: boolean;
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
    <h2 id="breakdown-heading">Focus breakdown</h2>
    <p class="muted">Loaded range{range ? " · " + captureTime(range.start_ms, timezone) + " – " + captureTime(range.end_ms, timezone) : ""}</p>
  </div>
  {#if totals && totals.focus_ms > 0}
    <dl id="totals" class="focus-totals">
      <div><dt>Recorded focus</dt><dd>{duration(totals.focus_ms)}</dd></div>
      <div><dt>Idle within focus</dt><dd>{duration(totals.idle_ms)}</dd></div>
      {#if configured}<div><dt>Categorized</dt><dd>{categorizedPercent.toFixed(0)}%</dd></div>{/if}
      <div><dt>Applications</dt><dd>{applications.length}</dd></div>
    </dl>
    <div class="breakdown-columns">
      <section aria-labelledby="categories-heading">
        <h3 id="categories-heading">Categories <span class="muted">· share of recorded focus</span></h3>
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
        <h3 id="applications-heading">Applications <span class="muted">· most time first</span></h3>
        {@render ranking(applications, "applications")}
      </section>
    </div>
    <details class="breakdown-details">
      <summary>Window titles <span class="muted">({titles.length})</span></summary>
      {@render ranking(titles, "titles")}
    </details>
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
    <p class="muted breakdown-note">Time with a window in focus, including idle time. Percentages are shares of recorded focus; recording gaps are excluded. Categories follow your rules over application and window title. This is observed activity, not time assigned to tasks.</p>
  {:else if totals}
    <p class="breakdown-empty">No recorded focus in this range.</p>
    <p class="muted">Choose another day or widen the timeline window to explore more activity.</p>
  {/if}
</section>
