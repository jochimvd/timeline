<script lang="ts">
  import { color, captureTime } from "./format";
  import type { Coverage, Overview, Range } from "./types";
  let { rows, totals, range, timezone, loading }: {
    rows: Coverage[]; totals?: Overview["totals"]; range?: Range; timezone: string; loading: boolean;
  } = $props();
  const ranked = (kind: string) => rows.filter(row => row.kind === kind)
    .sort((a, b) => b.elapsed_ms! - a.elapsed_ms!);
  const applications = $derived(ranked("application"));
  const titles = $derived(ranked("exact_title"));
  function duration(ms: number) {
    if (ms > 0 && ms < 60_000) return "<1 min";
    const minutes = Math.round(ms / 60_000);
    return minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)}h${minutes % 60 ? ` ${minutes % 60}m` : ""}`;
  }
</script>

{#snippet ranking(items: Coverage[], id: string)}
  <ol {id} class="focus-ranking">
    {#each items as row}
      <li>
        <div class="focus-row">
          <span class="focus-name">
            <span class="app-dot" style:background={color(row.application!)} aria-hidden="true"></span>
            <span>{row.title === undefined ? row.application : row.title || "Untitled window"}
              {#if row.title !== undefined}<small>{row.application}</small>{/if}
            </span>
          </span>
          <span class="focus-value">{duration(row.elapsed_ms!)} <span>{row.percent_of_focus!.toFixed(1)}%</span></span>
        </div>
        <div class="focus-bar-track" aria-hidden="true">
          <div style:width={row.percent_of_focus + "%"} style:background={color(row.application!)}></div>
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
      <div><dt>Applications</dt><dd>{applications.length}</dd></div>
    </dl>
    <h3>Applications <span class="muted">· most time first</span></h3>
    {@render ranking(applications, "applications")}
    <details class="window-breakdown">
      <summary>Window titles <span class="muted">({titles.length})</span></summary>
      {@render ranking(titles, "titles")}
    </details>
    <p class="muted breakdown-note">Time with a window in focus, including idle time. Percentages are shares of recorded focus; recording gaps are excluded. This is observed activity, not time assigned to tasks.</p>
  {:else if totals}
    <p class="breakdown-empty">No recorded focus in this range.</p>
    <p class="muted">Choose another day or widen the timeline window to explore more activity.</p>
  {/if}
</section>
