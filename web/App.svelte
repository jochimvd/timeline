<script lang="ts">
  import { onMount } from "svelte";
  import Timeline from "./Timeline.svelte";
  import StorageDetails from "./StorageDetails.svelte";
  import Breakdown from "./Breakdown.svelte";
  import EvidenceViewer from "./EvidenceViewer.svelte";
  import { createTimelineApp } from "./model";
  const app = createTimelineApp();
  const view = app.state;
  onMount(() => { void app.start(); return app.dispose; });
</script>

<main>
  <header class="app-header">
    <div><span class="eyebrow">DESKTOP ACTIVITY</span><h1>Your activity, recalled.</h1></div>
    <section class="status" aria-label="Recording">
      <span id="recording">{$view.recording}</span>
      <button id="pause" disabled={$view.recordingBusy} onclick={() => void app.toggleRecording()}>{$view.paused ? "Resume recording" : "Pause recording"}</button>
    </section>
  </header>
  <p id="message" role="status">{$view.message}</p>
  <section class="panel">
    <Timeline view={$view} {app} />
  </section>
  <EvidenceViewer view={$view} {app} />
  <Breakdown rows={$view.rows} categories={$view.categories} totals={$view.totals} range={$view.loadedRange}
    timezone={$view.timezone} settingsPath={$view.settingsPath} loading={$view.dayLoading || $view.rangeLoading} />
  <StorageDetails />
  <footer>Activity days begin at 04:00. Closing this page leaves recording running.</footer>
</main>
