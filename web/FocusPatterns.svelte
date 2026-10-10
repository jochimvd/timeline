<script lang="ts">
  import { clockRange, duration } from "./format";
  import { plural, stretchDistribution } from "./metrics";
  import type { FocusMetrics } from "./types";
  let { metrics, timezone, categorized }: { metrics: FocusMetrics; timezone: string; categorized: boolean } = $props();
  const buckets = $derived(stretchDistribution(metrics));
  const hours = $derived(metrics.hours.filter(hour => hour.active_ms > 0 || hour.application_switches > 0
    || (hour.category_switches ?? 0) > 0));
  const describe = (bucket: (typeof buckets)[number]) =>
    `${bucket.label}: ${duration(bucket.elapsed_ms)}, ${bucket.percent.toFixed(0)}% in ${plural(bucket.count, "stretch", "stretches")}`;
</script>

<section class="focus-patterns" aria-labelledby="stretches-heading">
  <h3 id="stretches-heading">Focus stretches <span class="muted">· active focus by stretch length</span></h3>
  <!-- Ordinal ramp: longer stretches take the stronger step. -->
  <div class="composition" role="img" aria-label={"Active focus by stretch length: " + buckets.map(describe).join("; ")}>
    {#each buckets as bucket (bucket.key)}
      {#if bucket.elapsed_ms > 0}
        <div style:flex-grow={bucket.elapsed_ms} style:background={`var(--ramp-${bucket.ramp})`} title={describe(bucket)}></div>
      {/if}
    {/each}
  </div>
  <ul class="stretch-legend" id="stretch-distribution">
    {#each buckets as bucket (bucket.key)}
      <li class:zero={bucket.count === 0}>
        <span class="swatch" style:background={`var(--ramp-${bucket.ramp})`} aria-hidden="true"></span>
        <span class="stretch-label">{bucket.label}<small>{plural(bucket.count, "stretch", "stretches")}</small></span>
        <span class="focus-value">{bucket.elapsed_ms ? duration(bucket.elapsed_ms) : "—"} <span>{bucket.percent.toFixed(0)}%</span></span>
      </li>
    {/each}
  </ul>
  {#if hours.length}
    <details class="breakdown-details hourly-details">
      <summary>Switches by hour <span class="muted">({hours.length})</span></summary>
      <div class="table-scroll">
        <table class="data-table" id="hourly-switches">
          <thead>
            <tr><th scope="col">Hour</th><th scope="col">Active focus</th><th scope="col">Application switches</th>
              {#if categorized}<th scope="col">Category switches</th>{/if}</tr>
          </thead>
          <tbody>
            {#each hours as hour (hour.range.start_ms)}
              <tr><th scope="row">{clockRange(hour.range, timezone)}</th><td>{duration(hour.active_ms)}</td>
                <td>{hour.application_switches}</td>{#if categorized}<td>{hour.category_switches ?? 0}</td>{/if}</tr>
            {/each}
          </tbody>
        </table>
      </div>
    </details>
  {/if}
</section>
