<script lang="ts">
  import Timeline from "./Timeline.svelte";
  import Breakdown from "./Breakdown.svelte";
  import EvidenceViewer from "./EvidenceViewer.svelte";
  import type { TimelineApp } from "./model";
  import type { Range } from "./types";
  let { app }: { app: TimelineApp } = $props();
  const view = $derived(app.state);
  /** Shows a range such as the longest stretch on the timeline, moving focus there. */
  function reveal(range: Range) {
    const timeline = document.getElementById("timeline");
    timeline?.focus({ preventScroll: true });
    timeline?.scrollIntoView({ behavior: "smooth", block: "center" });
    void app.reveal(range);
  }
</script>

<section class="panel" aria-label="Timeline">
  <Timeline view={$view} {app} />
</section>
<EvidenceViewer view={$view} {app} />
<Breakdown rows={$view.rows} categories={$view.categories} totals={$view.totals} metrics={$view.metrics}
  range={$view.loadedRange} timezone={$view.timezone} settingsPath={$view.settingsPath}
  loading={$view.dayLoading || $view.rangeLoading} onreveal={reveal} />
