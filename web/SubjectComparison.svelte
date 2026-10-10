<script lang="ts">
  import { duration, type Swatch } from "./format";
  import { durationDelta, type Subject, type SubjectChange } from "./trends";
  /** Active focus per subject in this period, with the comparison period as a
   *  tick on the same scale. */
  let { rows, swatch, heading, current, previous, comparing, missing }: {
    rows: SubjectChange[]; swatch: (subject: Subject) => Swatch; heading: string;
    /** Names of the two periods, such as "This week" and "last week". */
    current: string; previous: string;
    comparing: boolean; missing: boolean;
  } = $props();
  const max = $derived(Math.max(1, ...rows.flatMap(row => [row.current, row.previous ?? 0])));
  const delta = (row: SubjectChange) => row.previous === null ? null : durationDelta(row.current, row.previous, "");
</script>

<section class="subject-comparison" aria-labelledby="subjects-heading">
  <h3 id="subjects-heading">{heading} <span class="muted">· active focus</span></h3>
  <ul class="legend comparison-legend" aria-label="Marks">
    <li><span class="swatch comparison-bar-key"></span>{current}</li>
    <li><span class="comparison-tick-key"></span>{previous[0].toUpperCase() + previous.slice(1)}</li>
  </ul>
  {#if rows.length}
    <ol class="focus-ranking comparison-ranking" id="subject-comparison">
      {#each rows as row (row.subject.key)}
        {@const change = delta(row)}
        <li class:zero={row.current === 0}>
          <div class="focus-row">
            <span class="focus-name">
              <span class="app-dot" style:background={swatch(row.subject).fill} aria-hidden="true"></span>
              <span>{row.subject.name}</span>
            </span>
            <span class="focus-value">{row.current ? duration(row.current) : "—"}
              <span class="comparison-delta">
                {#if missing}—{:else if change}<span aria-hidden="true">{change.direction > 0 ? "▲" : change.direction < 0 ? "▼" : "="}</span>
                  {change.text}<span class="visually-hidden"> vs {previous} ({duration(row.previous ?? 0)})</span>
                {:else if comparing}…{/if}
              </span>
            </span>
          </div>
          <div class="focus-bar-track comparison-track" aria-hidden="true">
            <div style:width={100 * row.current / max + "%"} style:background={swatch(row.subject).fill}></div>
            {#if row.previous !== null && !missing}<span class="comparison-tick" style:left={100 * row.previous / max + "%"}></span>{/if}
          </div>
        </li>
      {/each}
    </ol>
  {:else}<p class="empty-hint">No active focus in this period.</p>{/if}
</section>
