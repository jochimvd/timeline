<script lang="ts">
  import Icon from "./Icon.svelte";
  import ScreenshotCanvas from "./ScreenshotCanvas.svelte";
  import { tick } from "svelte";
  import { captureTime, elapsed, imageTransform, noScreenshot, time } from "./format";
  import type { ImageFrame } from "./images";
  import type { TimelineApp, View } from "./model";
  import { initial, type Painter } from "./paint";
  import { gapLabel } from "./trends";
  let { view, app, paint }: { view: View; app: TimelineApp; paint: Painter } = $props();
  let dialog: HTMLDialogElement;
  let enlarged = $state.raw<ImageFrame | null>(null);
  let nativeSize = $state(false);
  const frames = $derived(view.frames);
  const monitors = $derived(Array.from(new Set(view.shots.map(s => s.screenshot.monitor))).sort());
  const single = $derived(view.layout.startsWith("monitor:") ? view.layout.slice(8) : null);
  const focus = $derived(view.focus.find(row => row.focus)?.focus);
  $effect(() => {
    // An open preview follows the selection on its monitor and closes without it.
    const current = frames, open = enlarged;
    if (!dialog?.open || !open) return;
    const next = current?.find(frame => frame.shot.screenshot.monitor === open.shot.screenshot.monitor);
    if (next?.draw) { if (next !== open) enlarged = next; }
    else { dialog.close(); enlarged = null; }
  });
  async function enlarge(frame: ImageFrame) { nativeSize = false; enlarged = frame; await tick(); dialog.showModal(); }
  function close() { dialog.close(); enlarged = null; }
  function previewKey(event: KeyboardEvent) {
    const direction = { ArrowLeft: -1, ArrowRight: 1, ",": -1, ".": 1 }[event.key];
    if (!direction) return;
    event.preventDefault();
    void app.stepScreenshot(direction, event.key === "," || event.key === ".");
  }
  const skip = $derived(view.skip?.at === view.selected ? view.skip : null);
  async function zoomImage(event: MouseEvent) {
    const button = event.currentTarget as HTMLButtonElement;
    const image = button.querySelector("canvas")!;
    const viewport = button.parentElement!;
    const before = image.getBoundingClientRect();
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
  const label = (frame: ImageFrame) => frame.shot.screenshot.monitor + " at " + time(frame.shot.screenshot.unix_ms, view.timezone);
  const describe = (kind: string | undefined, reason: string | undefined) =>
    kind === "gap" ? "Not recording · " + gapLabel(reason ?? "") : kind === "no_focus" ? "No focused window" : "No focus evidence at this time";
</script>

<section class="stage card" aria-label="Selected moment">
  <header class="head">
    <div class="when">
      <h2 id="selected" class="num">{view.evidenceTime === null ? "No moment selected" : time(view.evidenceTime, view.timezone)}</h2>
      {#if view.evidenceLoading}<span class="spinner" aria-label="Loading"></span>{/if}
      {#if skip}
        <span class="skip" title="Screenshots with only a few small changes, like a clock, are passed over. Use , and . to step one screenshot.">
          skipped {skip.count} unchanged · {elapsed(skip.ms)}</span>
      {/if}
    </div>
    <div class="actions">
      {#if monitors.length > 1 || single}
        <div class="segmented" role="group" aria-label="Monitors">
          <button aria-pressed={!single} onclick={() => void app.layout("grid")}>All</button>
          {#each monitors as monitor (monitor)}
            <button aria-pressed={single === monitor} onclick={() => void app.layout("monitor:" + monitor)}><Icon name="monitor" size={13} />{monitor}</button>
          {/each}
          {#if single && !monitors.includes(single)}<button aria-pressed="true" disabled>{single} (unavailable)</button>{/if}
        </div>
      {/if}
      <button class="btn small icon ghost" aria-label="Enlarge screenshot" title="Enlarge"
        disabled={!frames?.some(frame => frame.draw)} onclick={() => { const frame = frames?.find(f => f.draw); if (frame) void enlarge(frame); }}>
        <Icon name="expand" size={14} /></button>
    </div>
  </header>

  <div id="images" class="viewport" class:dim={view.evidenceLoading} class:multi={(frames?.length ?? 0) > 1} aria-busy={view.evidenceLoading}>
    <button class="nav prev" aria-label="Previous change" title="Previous change (←) · Alt-click or , for one screenshot" disabled={view.dayLoading}
      onclick={event => void app.stepScreenshot(-1, event.altKey)}>
      <Icon name="chevron-left" size={22} /></button>
    {#if view.evidenceError}<p class="empty error" role="alert">{view.evidenceError}</p>{/if}
    {#if frames === null}
      <div class="empty">
        <Icon name="now" size={28} />
        <p>Pick a moment on the timeline</p>
        <small>Hover to preview, click to select, ← → to step through screenshots</small>
      </div>
    {:else}
      {#each frames as frame (frame.shot.screenshot.monitor)}
        <figure>
          {#if frame.draw}
            <button class="shot" aria-label={"Enlarge " + frame.shot.screenshot.monitor} onclick={() => void enlarge(frame)}>
              <ScreenshotCanvas {frame} label={label(frame)} transform={imageTransform(frame.shot.screenshot.transform)} />
            </button>
          {:else if frame.error}<p class="empty error">{frame.error}</p>{/if}
          {#if frames.length > 1 || frame.shot.stale}
            <figcaption>
              <span>{frame.shot.screenshot.monitor}</span>
              {#if frame.shot.stale}<span class="badge warn" title={"Captured " + captureTime(frame.shot.screenshot.unix_ms, view.timezone)}>from {time(frame.shot.screenshot.unix_ms, view.timezone)}</span>{/if}
            </figcaption>
          {/if}
        </figure>
      {:else}
        <div class="empty">
          <Icon name="image-off" size={28} />
          <p>{noScreenshot(view.shotsExpired)}</p>
        </div>
      {/each}
    {/if}
    <button class="nav next" aria-label="Next change" title="Next change (→) · Alt-click or . for one screenshot" disabled={view.dayLoading}
      onclick={event => void app.stepScreenshot(1, event.altKey)}>
      <Icon name="chevron-right" size={22} /></button>
  </div>

  <footer id="focus" class="focus">
    {#if focus?.kind === "focus" && focus.window}
      {@const owner = paint.application(focus.window.application)}
      {@const category = paint.category(focus.category ?? null)}
      <span class="avatar" style:background={owner.swatch.fill} style:color={owner.swatch.ink} aria-hidden="true">{initial(focus.window.application)}</span>
      <div class="focus-text">
        <p class="window-title ellipsis" title={focus.window.title}>{focus.window.title || "Untitled window"}</p>
        <p class="window-app">{focus.window.application}{#if focus.idle} · <span class="badge">idle</span>{/if}</p>
      </div>
      {#if paint.mode === "category" || focus.category}
        <span class="pill" title={category.detail}><span class="dot" style:background={category.swatch.fill}></span>{category.detail}</span>
      {/if}
    {:else if focus}
      <span class="avatar none" aria-hidden="true"><Icon name="info" size={16} /></span>
      <p class="window-app">{describe(focus.kind, focus.reason)}</p>
    {:else if frames !== null}
      <p class="window-app">No focus evidence at this time.</p>
    {/if}
  </footer>
</section>

<dialog id="enlarged" aria-label="Screenshot preview" bind:this={dialog} onclose={() => enlarged = null} onkeydown={previewKey}>
  <button class="backdrop" aria-label="Dismiss screenshot preview" tabindex="-1" onclick={close}></button>
  <div class="lightbox">
    <div class="lightbox-bar">
      {#if enlarged}<span class="num">{enlarged.shot.screenshot.monitor} · {captureTime(enlarged.shot.screenshot.unix_ms, view.timezone)}</span>{/if}
      <span class="muted">← → step · click to zoom</span>
      <button id="close-image" class="btn icon ghost" aria-label="Close screenshot preview" onclick={close}><Icon name="x" /></button>
    </div>
    <div id="large-image" class:native-size={nativeSize}>
      {#if enlarged?.draw}
        <button class="image-zoom" aria-label={nativeSize ? "Fit image to window" : "Zoom image to native resolution"}
          aria-pressed={nativeSize} onclick={zoomImage}>
          <ScreenshotCanvas frame={enlarged} label={label(enlarged)} transform={imageTransform(enlarged.shot.screenshot.transform)} />
        </button>
      {/if}
    </div>
  </div>
</dialog>

<style>
  .stage { display: flex; flex-direction: column; overflow: hidden; min-height: 0; }
  .head, .focus { flex: none; }
  .head { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 10px 14px; border-bottom: 1px solid var(--border); }
  .when { display: flex; align-items: center; gap: 10px; }
  .when h2 { font-size: 15px; font-weight: 600; letter-spacing: -0.01em; }
  .actions { display: flex; align-items: center; gap: 8px; }
  .skip { font-size: 12px; color: var(--muted); white-space: nowrap; }
  .viewport {
    position: relative; flex: 1; min-height: 0; display: flex; gap: 12px; align-items: center; justify-content: center;
    padding: 12px; background: var(--sunken);
    background-image: radial-gradient(circle at 1px 1px, color-mix(in oklab, var(--text), transparent 93%) 1px, transparent 0);
    background-size: 18px 18px;
  }
  .viewport.dim figure { opacity: 0.6; transition: opacity 0.2s 0.2s; }
  figure {
    position: relative; flex: 1 1 0; min-width: 0; height: 100%; display: flex; flex-direction: column;
    align-items: center; justify-content: center; gap: 6px;
  }
  .shot { display: flex; min-height: 0; padding: 0; border: 0; background: none; cursor: zoom-in; border-radius: 6px; max-width: 100%; }
  .shot :global(canvas) {
    display: block; max-width: 100%; max-height: 100%; width: auto; height: auto; object-fit: contain; border-radius: 6px;
    box-shadow: 0 0 0 1px var(--border-strong), 0 18px 40px -16px rgb(0 0 0 / 0.7);
  }
  figcaption { display: flex; gap: 8px; align-items: center; font-size: 12px; color: var(--muted); }
  .empty { display: flex; flex-direction: column; align-items: center; gap: 8px; color: var(--muted); text-align: center; }
  .empty p { color: var(--text-2); font-weight: 500; }
  .empty.error, p.error { color: var(--bad); }
  .nav {
    position: absolute; top: 50%; transform: translateY(-50%); z-index: 1; display: grid; place-items: center;
    width: 36px; height: 56px; border-radius: 10px; border: 1px solid var(--border-strong);
    background: color-mix(in oklab, var(--surface), transparent 25%); backdrop-filter: blur(6px);
    box-shadow: 0 6px 18px -6px rgb(0 0 0 / 0.5); color: var(--text-2); cursor: pointer; opacity: 0; transition: opacity 0.15s;
  }
  .viewport:hover .nav, .nav:focus-visible { opacity: 1; }
  .nav:hover { color: var(--text); background: var(--surface-2); }
  .prev { left: 20px; }
  .next { right: 20px; }
  .focus { display: flex; align-items: center; gap: 12px; padding: 12px 14px; min-height: 64px; border-top: 1px solid var(--border); }
  .avatar {
    display: grid; place-items: center; width: 34px; height: 34px; border-radius: 9px; flex: none;
    font-weight: 700; font-size: 15px;
  }
  .avatar.none { background: var(--surface-3); color: var(--muted); }
  .focus-text { flex: 1; min-width: 0; }
  .window-title { font-weight: 550; color: var(--text); }
  .window-app { font-size: 12.5px; color: var(--muted); display: flex; align-items: center; gap: 4px; }

  dialog { padding: 0; border: 0; background: transparent; max-width: none; max-height: none; width: 100vw; height: 100vh; }
  dialog::backdrop { background: rgb(0 0 0 / 0.85); backdrop-filter: blur(4px); }
  .backdrop { position: fixed; inset: 0; border: 0; background: transparent; cursor: zoom-out; }
  .lightbox { position: relative; display: flex; flex-direction: column; height: 100%; padding: 12px 20px 20px; pointer-events: none; }
  .lightbox > * { pointer-events: auto; }
  .lightbox-bar { display: flex; align-items: center; gap: 16px; color: #eee; font-size: 13px; margin-bottom: 10px; align-self: stretch; }
  .lightbox-bar .muted { flex: 1; color: #999; }
  .lightbox-bar .btn { color: #eee; }
  #large-image { flex: 1; min-height: 0; display: grid; place-items: center; overflow: auto; }
  .image-zoom { padding: 0; border: 0; background: none; cursor: zoom-in; display: grid; place-items: center; max-width: 100%; max-height: 100%; }
  .image-zoom :global(canvas) { display: block; max-width: calc(100vw - 40px); max-height: calc(100vh - 80px); border-radius: 4px; }
  .native-size { place-items: start !important; }
  .native-size .image-zoom { cursor: zoom-out; max-width: none; max-height: none; }
  .native-size .image-zoom :global(canvas) { max-width: none; max-height: none; }
  @media (max-width: 700px) {
    .viewport { padding: 10px; } .nav { display: none; }
    .viewport.multi { flex-direction: column; }
    figure { height: auto; width: 100%; }
    .head { flex-wrap: wrap; }
  }
</style>
