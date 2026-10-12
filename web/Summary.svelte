<script lang="ts">
  import { clockRange, duration } from "./format";
  import type { Painter } from "./paint";
  import type { CategoryRow, Coverage, Range } from "./types";
  /** Where the visible range's focus went: categories, applications and windows. */
  let { rows, categories, paint, range, whole, timezone, loading }: {
    rows: Coverage[]; categories: CategoryRow[]; paint: Painter; range?: Range;
    /** Whether the timeline shows the whole day. */
    whole: boolean; timezone: string; loading: boolean;
  } = $props();
  type Entry = { key: string; name: string; detail?: string; ms: number; percent: number; fill: string; depth: number };
  const ranked = (kind: string) => rows.filter(row => row.kind === kind).sort((a, b) => b.elapsed_ms! - a.elapsed_ms!);
  const applications = $derived(ranked("application").map((row): Entry => ({
    key: row.application!, name: row.application!, ms: row.elapsed_ms!, percent: row.percent_of_focus!,
    fill: paint.application(row.application!).swatch.fill, depth: 0,
  })));
  const titles = $derived(ranked("exact_title").map((row): Entry => ({
    key: row.application + "\n" + row.title, name: row.title || "Untitled window", detail: row.application!,
    ms: row.elapsed_ms!, percent: row.percent_of_focus!, fill: paint.application(row.application!).swatch.fill, depth: 0,
  })));
  /** The category tree, siblings by time, uncategorized last. */
  const tree = $derived.by(() => {
    const children = new Map<string | null, CategoryRow[]>();
    for (const row of categories) children.set(row.parent, [...children.get(row.parent) ?? [], row]);
    const rank = (list: CategoryRow[]) => [...list].sort((a, b) => Number(a.category === null) - Number(b.category === null) || b.elapsed_ms - a.elapsed_ms);
    const walk = (list: CategoryRow[]): Entry[] => rank(list).filter(row => row.elapsed_ms > 0).flatMap(row => [{
      key: "c:" + (row.category ?? ""), name: row.category ?? "Uncategorized", ms: row.elapsed_ms, percent: row.percent_of_focus,
      fill: paint.category(row.category).swatch.fill, depth: row.depth,
      detail: row.own_ms > 0 && row.own_ms < row.elapsed_ms ? `${duration(row.own_ms)} not in a subcategory` : undefined,
    }, ...(row.category === null ? [] : walk(children.get(row.category) ?? []))]);
    return walk(children.get(null) ?? []);
  });
  // The composition bar follows the color mode.
  const composition = $derived(paint.mode === "category" ? tree.filter(entry => entry.depth === 0) : applications.slice(0, 8));
  const total = $derived(composition.reduce((sum, entry) => sum + entry.ms, 0));
  let moreApps = $state(false), moreTitles = $state(false);
</script>

{#snippet list(entries: Entry[], id: string)}
  {@const max = Math.max(1, ...entries.map(e => e.ms))}
  <ul class="rows" {id}>
    {#each entries as entry (entry.key)}
      <li style:padding-left={entry.depth * 16 + "px"}>
        <div class="line">
          <span class="dot" style:background={entry.fill}></span>
          <span class="name ellipsis" title={entry.name}>{entry.name}</span>
          <span class="value num">{duration(entry.ms)}</span>
          <span class="share num">{entry.percent.toFixed(0)}%</span>
        </div>
        {#if entry.detail}<div class="detail ellipsis">{entry.detail}</div>{/if}
        <div class="bar"><span style:width={100 * entry.ms / max + "%"} style:background={entry.fill}></span></div>
      </li>
    {/each}
  </ul>
{/snippet}

<div class="summary" class:stale={loading}>
  <p class="scope muted">{whole ? "Whole day" : range ? `Visible range · ${clockRange(range, timezone)}` : ""}</p>
  {#if composition.length && total > 0}
    <div class="composition" role="img" aria-label="Share of focus">
      {#each composition as entry (entry.key)}
        <span style:flex-grow={entry.ms} style:background={entry.fill} title={`${entry.name} · ${duration(entry.ms)}`}></span>
      {/each}
    </div>
  {/if}
  {#if tree.length}
    <section>
      <h3 class="eyebrow">Categories</h3>
      {@render list(tree, "category-breakdown")}
    </section>
  {/if}
  <section>
    <h3 class="eyebrow">Applications</h3>
    {#if applications.length}
      {@render list(moreApps ? applications : applications.slice(0, 6), "application-breakdown")}
      {#if applications.length > 6}<button class="link more" onclick={() => moreApps = !moreApps}>{moreApps ? "Show fewer" : `Show all ${applications.length}`}</button>{/if}
    {:else}<p class="muted empty">No focus in this range.</p>{/if}
  </section>
  {#if titles.length}
    <section>
      <h3 class="eyebrow">Windows</h3>
      {@render list(moreTitles ? titles.slice(0, 100) : titles.slice(0, 8), "title-breakdown")}
      {#if titles.length > 8}<button class="link more" onclick={() => moreTitles = !moreTitles}>{moreTitles ? "Show fewer" : `Show more (${titles.length})`}</button>{/if}
    </section>
  {/if}
</div>

<style>
  .summary { display: flex; flex-direction: column; gap: 18px; transition: opacity 0.2s; }
  .summary.stale { opacity: 0.6; }
  .scope { font-size: 12px; }
  .composition { display: flex; gap: 2px; height: 10px; border-radius: 5px; overflow: hidden; margin-top: -8px; }
  .composition span { min-width: 2px; }
  section { display: flex; flex-direction: column; gap: 8px; }
  .rows { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 9px; }
  .line { display: flex; align-items: center; gap: 8px; font-size: 13px; }
  .name { flex: 1; }
  .value { color: var(--text); font-size: 12.5px; }
  .share { color: var(--muted); font-size: 12px; width: 34px; text-align: right; }
  .detail { font-size: 11.5px; color: var(--muted); padding-left: 16px; }
  .bar { height: 3px; margin: 4px 0 0 16px; border-radius: 2px; background: var(--surface-3); overflow: hidden; }
  .bar span { display: block; height: 100%; border-radius: 2px; }
  .more { align-self: flex-start; font-size: 12.5px; }
  .empty { font-size: 13px; }
</style>
