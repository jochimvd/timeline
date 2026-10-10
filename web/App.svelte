<script lang="ts">
  import { onMount } from "svelte";
  import DayView from "./DayView.svelte";
  import StorageDetails from "./StorageDetails.svelte";
  import { createTimelineApp } from "./model";
  import { createRecording } from "./recording";
  import { startSession } from "./session";
  const app = createTimelineApp();
  const day = app.state;
  const recording = createRecording();
  const status = recording.state;
  let error = $state("");
  onMount(() => {
    const lifetime = new AbortController();
    startSession(lifetime.signal).then(session => {
      void app.start(session);
      void recording.start();
    }).catch(failure => {
      if (!lifetime.signal.aborted) error = failure instanceof Error ? failure.message : String(failure);
    });
    return () => { lifetime.abort(); app.dispose(); recording.dispose(); };
  });
</script>

<main>
  <header class="app-header">
    <div><span class="eyebrow">DESKTOP ACTIVITY</span><h1>Your activity, recalled.</h1></div>
    <section class="status" aria-label="Recording">
      <span id="recording">{$status.label}</span>
      <button id="pause" disabled={$status.busy} onclick={() => void recording.toggle()}>{$status.paused ? "Resume recording" : "Pause recording"}</button>
    </section>
  </header>
  <p id="message" role="status">{error || $status.error || $day.message}</p>
  <DayView {app} />
  <StorageDetails />
  <footer>Activity days begin at 04:00. Closing this page leaves recording running.</footer>
</main>
