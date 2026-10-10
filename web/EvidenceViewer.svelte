<script lang="ts">
  import ScreenshotCanvas from "./ScreenshotCanvas.svelte";
  import SuspenseBoundary from "./SuspenseBoundary.svelte";
  import { tick } from "svelte";
  import { categoryLabel, categoryNodes, categorySwatches } from "./categories";
  import { time, captureTime, imageTransform, noScreenshot, swatchStyle } from "./format";
  import type { TimelineApp, View } from "./model";
  import type { ImageFrame } from "./images";
  let { view, app }: { view: View; app: TimelineApp } = $props();
  let dialog: HTMLDialogElement;
  let enlarged = $state.raw<ImageFrame | null>(null);
  let nativeSize = $state(false);
  const frames = $derived(view.frames);
  const nodes = $derived(categoryNodes(view.categories));
  const configured = $derived(nodes.length > 0);
  const categorySwatchOf = $derived(categorySwatches(nodes));
  const monitors = $derived(Array.from(new Set(view.shots.map(s => s.screenshot.monitor))));
  const missingMonitor = $derived(view.layout.startsWith("monitor:") && !monitors.includes(view.layout.slice(8)));
  $effect(() => {
    // Keep an open preview mounted as selections change. Only a completed
    // selection without this monitor closes it.
    const currentFrames = frames, currentEnlarged = enlarged;
    if (!dialog?.open || !currentEnlarged) return;
    const next = currentFrames?.find(frame => frame.shot.screenshot.monitor === currentEnlarged.shot.screenshot.monitor);
    if (next?.draw) { if (next !== currentEnlarged) enlarged = next; }
    else { dialog.close(); enlarged = null; }
  });
  function transform(frame: ImageFrame) {
    return imageTransform(frame.shot.screenshot.transform);
  }
  function caption(frame: ImageFrame) {
    const s = frame.shot.screenshot;
    return s.monitor + " · " + captureTime(s.unix_ms, view.timezone) + (frame.shot.stale ? " · stale" : "");
  }
  async function enlarge(frame: ImageFrame) { nativeSize = false; enlarged = frame; await tick(); dialog.showModal(); }
  function close() { dialog.close(); enlarged = null; }
  function previewKey(event: KeyboardEvent) {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    void app.stepScreenshot(event.key === "ArrowLeft" ? -1 : 1);
  }
  async function zoomImage(event: MouseEvent) {
    const button = event.currentTarget as HTMLButtonElement;
    const image = button.querySelector("canvas")!;
    const viewport = button.parentElement!;
    const before = image.getBoundingClientRect();
    // Keyboard activation zooms to the center; pointer activation keeps its target.
    const x = event.detail === 0 ? 0.5 : Math.max(0, Math.min(1, (event.clientX - before.left) / before.width));
    const y = event.detail === 0 ? 0.5 : Math.max(0, Math.min(1, (event.clientY - before.top) / before.height));
    nativeSize = !nativeSize;
    await tick();
    if (!nativeSize) { viewport.scrollTo(0, 0); return; }
    const after = image.getBoundingClientRect();
    const bounds = viewport.getBoundingClientRect();
    viewport.scrollBy({
      left: after.left + x * after.width - bounds.left - viewport.clientLeft - viewport.clientWidth / 2,
      top: after.top + y * after.height - bounds.top - viewport.clientTop - viewport.clientHeight / 2,
      behavior: "instant",
    });
  }
</script>

<section class="panel evidence-panel">
  <div class="toolbar">
    <h2 id="selected">{view.evidenceTime === null ? "Select a moment" : captureTime(view.evidenceTime, view.timezone)}</h2>
    <label>Monitors
      <select id="layout" value={view.layout} onchange={event => void app.layout(event.currentTarget.value)}>
        <option value="grid">Grid</option>
        <option value="stack">Stack</option>
        {#each monitors as monitor (monitor)}<option value={"monitor:" + monitor}>{monitor}</option>{/each}
        {#if missingMonitor}<option value={view.layout}>{view.layout.slice(8)} (unavailable)</option>{/if}
      </select>
    </label>
  </div>
  <div id="focus">
    {#each view.focus as row}
      <p title={row.focus?.window?.title}>{row.focus?.kind === "focus"
        ? row.focus.window!.application + " · " + row.focus.window!.title + (row.focus.idle ? " · idle" : "")
        : "Focus: " + (row.focus?.reason || row.focus?.kind || "unknown")}{#if configured && row.focus?.kind === "focus"}
          <span class="category-tag"><span class="swatch" style={swatchStyle(categorySwatchOf(row.focus.category ?? null))}></span>{categoryLabel(nodes, row.focus.category ?? null)}</span>{/if}</p>
    {:else}
      {#if view.frames !== null}No focus evidence at this time.{/if}
    {/each}
  </div>
  <div id="images" class={view.layout === "stack" ? "stack" : "grid"} aria-busy={view.evidenceLoading}>
    {#if view.evidenceError}<p class="error evidence-error" role="alert">{view.evidenceError}</p>{/if}
    <SuspenseBoundary ready={frames !== null}>
      {#snippet pending()}
        <p class="muted" style:visibility={view.evidenceLoading ? "hidden" : "visible"}>Click the timeline or choose a screenshot to inspect a moment.</p>
      {/snippet}
      {#each frames ?? [] as frame (frame.shot.screenshot.monitor)}
        <figure>
          <figcaption title={caption(frame)}>{caption(frame)}</figcaption>
          {#if frame.draw}
            <button aria-label={"Enlarge " + frame.shot.screenshot.monitor} onclick={() => void enlarge(frame)}>
              <ScreenshotCanvas {frame} label={frame.shot.screenshot.monitor + " at " + time(frame.shot.screenshot.unix_ms, view.timezone)}
                transform={transform(frame)} />
            </button>
          {:else if frame.error}<p class="error">{frame.error}</p>{/if}
        </figure>
      {:else}<p class="empty">{noScreenshot(view.shotsExpired)}</p>{/each}
    </SuspenseBoundary>
  </div>
</section>
<dialog id="enlarged" aria-label="Screenshot preview" bind:this={dialog} onclose={() => enlarged = null} onkeydown={previewKey}>
  <button class="image-backdrop" aria-label="Dismiss screenshot preview" tabindex="-1" onclick={close}></button>
  <div class="image-preview">
  <div id="large-image" class:native-size={nativeSize}>
    {#if enlarged?.draw}
      <button class="image-zoom" aria-label={nativeSize ? "Fit image to window" : "Zoom image to native resolution"}
        aria-pressed={nativeSize} onclick={zoomImage}>
        <ScreenshotCanvas frame={enlarged} label={enlarged.shot.screenshot.monitor + " at " + time(enlarged.shot.screenshot.unix_ms, view.timezone)}
          transform={transform(enlarged)} />
      </button>
    {/if}
  </div>
  <button id="close-image" aria-label="Close screenshot preview" onclick={close}>×</button>
  </div>
</dialog>
