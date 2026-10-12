<script lang="ts">
  import Icon from "./Icon.svelte";
  import ScreenshotCanvas from "./ScreenshotCanvas.svelte";
  import { onMount } from "svelte";
  import { on } from "svelte/events";
  import { clusterCommits, commitCount, repositoryLabel, type CommitCluster } from "./commits";
  import { clock, clockRange, coverageBands, duration, formatter, imageTransform, time } from "./format";
  import { zoomLevels, type TimelineApp, type View } from "./model";
  import type { Painter } from "./paint";
  import { gapLabel } from "./trends";
  import type { Block, Coverage, DayEvidence, FocusSession, Range } from "./types";
  let { view, app, paint, sessions, evidence }: {
    view: View; app: TimelineApp; paint: Painter;
    /** The whole day's sessions and evidence, for the overview strip. */
    sessions: FocusSession[] | null; evidence: DayEvidence | null;
  } = $props();
  const visible = $derived(view.visible);
  const timezone = $derived(view.timezone);
  let width = $state(0);
  $effect(() => app.setWidth(width));
  const span = $derived(visible ? visible.end_ms - visible.start_ms : 1);
  const percent = (ms: number) => visible ? 100 * (ms - visible.start_ms) / span : 0;
  const inView = (ms: number) => !!visible && ms >= visible.start_ms && ms < visible.end_ms;
  const place = (range: Range) => `left:${percent(range.start_ms)}%;width:${Math.max(0, percent(range.end_ms) - percent(range.start_ms))}%`;
  const px = (range: Range) => width * (range.end_ms - range.start_ms) / span;

  /** Adjacent blocks painted alike form one band segment, labelled when wide enough. */
  type Segment = { range: Range; key: string; label: string; fill: string; ink: string; mixed: boolean; blocks: Block[] };
  const segments = $derived.by(() => {
    const result: Segment[] = [];
    for (const block of view.blocks) {
      const p = paint.focus(block.group.application, block.category);
      const last = result.at(-1);
      if (last && last.key === p.key && last.range.end_ms === block.range.start_ms && last.mixed === !!block.mixed) {
        last.range = { ...last.range, end_ms: block.range.end_ms };
        last.blocks.push(block);
      } else result.push({ range: { ...block.range }, key: p.key, label: p.label, fill: p.swatch.fill, ink: p.swatch.ink, mixed: !!block.mixed, blocks: [block] });
    }
    return result;
  });
  const focusBands = $derived(coverageBands(view.rows.filter(row => row.kind === "focus_coverage")));
  const gaps = $derived(focusBands.filter(row => row.focus?.kind !== "focus" || row.has_gap));
  const idle = $derived(focusBands.filter(row => row.idle || row.focus?.idle));
  const monitors = $derived(Array.from(new Set(view.rows.filter(row => row.kind === "image_coverage").map(row => row.monitor!))).sort());
  const shotBands = $derived(monitors.map(monitor => ({ monitor,
    bands: coverageBands(view.rows.filter(row => row.kind === "image_coverage" && row.monitor === monitor)) })));

  // Ticks on round local times, about one per 110 px.
  const STEPS = [5_000, 10_000, 15_000, 30_000, 60_000, 300_000, 600_000, 900_000, 1_800_000, 3_600_000, 7_200_000, 10_800_000, 14_400_000];
  const ticks = $derived.by(() => {
    if (!visible || width <= 0) return [];
    const step = STEPS.find(step => step * width / span >= 110) ?? STEPS.at(-1)!;
    const parts = formatter(timezone, "offset", { hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23" }).formatToParts(visible.start_ms);
    const value = (type: string) => Number(parts.find(part => part.type === type)?.value ?? 0);
    const local = ((value("hour") * 60 + value("minute")) * 60 + value("second")) * 1000;
    const first = visible.start_ms - local + Math.ceil(local / step) * step - (visible.start_ms % 1000);
    const result: number[] = [];
    for (let ms = first; ms < visible.end_ms && result.length < 60; ms += step) result.push(ms);
    return result;
  });
  const tickLabel = (ms: number) => span < 20 * 60_000 ? time(ms, timezone) : clock(ms, timezone);

  // Commits, clustered where markers would overlap.
  const commitList = $derived(view.commits);
  const showCommits = $derived(!!commitList?.configured || !!view.commitError);
  const clusters = $derived(visible && commitList ? clusterCommits(commitList.commits, visible, width) : []);

  // Hover, drag and wheel.
  let hoverTime = $state<number | null>(null);
  let pointerX = $state(0);
  let trackTop = $state(0), trackBottom = $state(0);
  /** The hover card has a fixed size, so its content never moves it. */
  const CARD_HEIGHT = 272;
  /** Above the track unless it would leave the viewport. */
  const below = $derived(trackTop - 12 - CARD_HEIGHT < 64);
  let hoverCluster = $state.raw<CommitCluster | null>(null);
  let dragging = $state(false);
  let gesture: { id: number; x: number; width: number; range: Range; date: string; zoom: string } | null = null;
  let suppressClick = false, lastWheelZoom = -Infinity;
  let panFrame = 0, pendingPan: number | null = null;
  let tracks: HTMLDivElement;
  let now = $state(Date.now());
  onMount(() => {
    const timer = setInterval(() => now = Date.now(), 1000);
    return () => { clearInterval(timer); cancelAnimationFrame(panFrame); };
  });
  function schedulePan(center: number) {
    pendingPan = center;
    if (!panFrame) panFrame = requestAnimationFrame(() => {
      panFrame = 0;
      const next = pendingPan; pendingPan = null;
      if (next !== null && gesture?.date === view.date && gesture.zoom === view.zoom) app.panTo(next);
    });
  }
  function fraction(event: MouseEvent) {
    const rect = tracks.getBoundingClientRect();
    return Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width));
  }
  const timeAt = (f: number) => Math.round(Math.min(visible!.end_ms - 1, visible!.start_ms + f * span));
  function move(event: PointerEvent) {
    if (!visible || view.dayLoading) return;
    if (gesture && event.pointerId === gesture.id) {
      if (gesture.date !== view.date || gesture.zoom !== view.zoom) { end(event); return; }
      const distance = event.clientX - gesture.x;
      if (Math.abs(distance) >= 4) dragging = true;
      if (dragging) {
        hoverTime = null;
        schedulePan((gesture.range.start_ms + gesture.range.end_ms) / 2 - distance / gesture.width * (gesture.range.end_ms - gesture.range.start_ms));
      }
      return;
    }
    const rect = tracks.getBoundingClientRect();
    pointerX = event.clientX;
    trackTop = rect.top;
    trackBottom = rect.bottom;
    hoverTime = timeAt(fraction(event));
    hoverCluster = clusterAt(event);
    // Over a marker, the hover snaps to the moment a click would select.
    if (hoverCluster) hoverTime = hoverCluster.commits[0].time_ms;
    app.hover(hoverTime);
  }
  /** The commit marker under the pointer: only the dot itself, not its time elsewhere on the timeline. */
  function clusterAt(event: MouseEvent) {
    const row = commitTrack?.getBoundingClientRect();
    if (!row || event.clientY < row.top || event.clientY > row.bottom) return null;
    const rect = tracks.getBoundingClientRect(), x = event.clientX - rect.left;
    const at = (ms: number) => (ms - visible!.start_ms) / span * rect.width;
    return clusters.find(c => at(c.start_ms) - 7 <= x && x <= at(c.end_ms) + 7) ?? null;
  }
  let commitTrack: HTMLDivElement | undefined = $state();
  function leave() { hoverTime = null; hoverCluster = null; app.leave(); }
  function down(event: PointerEvent) {
    if (!visible || view.dayLoading || event.button !== 0 || gesture) return;
    gesture = { id: event.pointerId, x: event.clientX, width: tracks.getBoundingClientRect().width,
      range: { ...visible }, date: view.date, zoom: view.zoom };
    suppressClick = false;
    tracks.setPointerCapture(event.pointerId);
  }
  function end(event: PointerEvent) {
    if (!gesture || gesture.id !== event.pointerId) return;
    suppressClick = dragging || event.type === "pointercancel";
    const same = gesture.date === view.date && gesture.zoom === view.zoom;
    const center = pendingPan ?? (visible ? (visible.start_ms + visible.end_ms) / 2 : null);
    cancelAnimationFrame(panFrame); panFrame = 0; pendingPan = null;
    gesture = null;
    if (dragging && same && center !== null) app.panTo(center, true);
    dragging = false;
    if (tracks.hasPointerCapture(event.pointerId)) tracks.releasePointerCapture(event.pointerId);
  }
  function click(event: MouseEvent) {
    if (suppressClick) { suppressClick = false; return; }
    if (!visible || view.dayLoading) return;
    const cluster = clusterAt(event);
    if (cluster) { void app.select(cluster.commits[0].time_ms); return; }
    void app.select(timeAt(fraction(event)));
  }
  function key(event: KeyboardEvent) {
    if (event.key === "Escape") { leave(); return; }
    if (!visible || view.dayLoading) return;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      event.stopPropagation();
      const direction = event.key === "ArrowRight" ? 1 : -1;
      if (event.shiftKey) app.pan(direction); else void app.stepScreenshot(direction);
    } else if (["+", "=", "-"].includes(event.key)) {
      event.preventDefault(); event.stopPropagation(); app.stepZoom(event.key === "-" ? -1 : 1);
    } else if (event.key === "Home") { event.preventDefault(); void app.jumpNow(); }
  }
  const zoomIndex = () => view.zoom === "fit"
    ? Math.max(0, zoomLevels.findIndex(level => (level.durationMs ?? 86_400_000 / Number(level.value)) <= view.fitWidth!))
    : zoomLevels.findIndex(level => level.value === view.zoom);
  function wheel(event: WheelEvent) {
    if (gesture || event.ctrlKey || !visible || view.dayLoading) return;
    if (Math.abs(event.deltaX) > Math.abs(event.deltaY)) {
      // Horizontal scrolling pans; the range settles once it stops.
      event.preventDefault();
      app.panTo((visible.start_ms + visible.end_ms) / 2 + event.deltaX / Math.max(1, width) * span);
      clearTimeout(settle);
      settle = setTimeout(() => visible && app.panTo((visible.start_ms + visible.end_ms) / 2, true), 200);
      return;
    }
    if (!event.deltaY) return;
    event.preventDefault();
    if (performance.now() - lastWheelZoom < 180) return;
    const index = zoomIndex();
    const next = Math.max(0, Math.min(zoomLevels.length - 1, index + (event.deltaY < 0 ? 1 : -1)));
    if (next === index) return;
    const f = fraction(event), ms = timeAt(f);
    lastWheelZoom = performance.now();
    app.zoom(zoomLevels[next].value, f, ms, true);
    hoverTime = ms;
    app.hover(ms);
  }
  let settle: ReturnType<typeof setTimeout> | undefined;
  function wheelEvents(element: HTMLElement) { return { destroy: on(element, "wheel", wheel, { passive: false }) }; }

  // Hover card content.
  const hoverSegment = $derived(hoverTime === null ? undefined
    : segments.find(s => s.range.start_ms <= hoverTime! && hoverTime! < s.range.end_ms));
  const hoverBlock = $derived(hoverSegment?.blocks.find(b => b.range.start_ms <= hoverTime! && hoverTime! < b.range.end_ms));
  const hoverGap = $derived(hoverTime === null ? undefined
    : focusBands.find(row => row.range && row.range.start_ms <= hoverTime! && hoverTime! < row.range.end_ms));
  const previewReady = $derived(hoverTime !== null && view.hoverEvidence.time === hoverTime);
  const exact = $derived(previewReady ? view.hoverEvidence.focus.find(row => row.focus)?.focus : undefined);
  const previewFrames = $derived(view.hoverEvidence.frames ?? []);
  const cardWidth = 320;
  let viewportWidth = $state(1200);
  const cardX = $derived(Math.max(8, Math.min(viewportWidth - cardWidth - 8, pointerX - cardWidth / 2)));
  function describeGap(row: Coverage | undefined) {
    if (!row?.focus) return "No evidence";
    if (row.focus.kind === "gap") return "Not recording · " + gapLabel(row.focus.reason ?? "");
    if (row.focus.kind === "no_focus") return "No focused window";
    if (row.focus.kind === "unknown") return "No evidence";
    return row.has_gap ? "Partly without evidence · zoom in" : "";
  }

  // Overview strip of the whole day, always shown so zooming never shifts the tracks.
  const daySpan = $derived(view.day ? view.day.end_ms - view.day.start_ms : 1);
  const dayPercent = (ms: number) => view.day ? Math.max(0, Math.min(100, 100 * (ms - view.day.start_ms) / daySpan)) : 0;
  let strip: HTMLDivElement | undefined = $state();
  let stripDrag: number | null = null;
  function stripTime(event: PointerEvent) {
    const rect = strip!.getBoundingClientRect();
    return view.day!.start_ms + Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)) * daySpan;
  }
  function stripDown(event: PointerEvent) {
    if (!view.day || view.dayLoading || event.button !== 0) return;
    stripDrag = event.pointerId;
    strip!.setPointerCapture(event.pointerId);
    app.panTo(stripTime(event));
  }
  function stripMove(event: PointerEvent) { if (stripDrag === event.pointerId) app.panTo(stripTime(event)); }
  function stripUp(event: PointerEvent) {
    if (stripDrag !== event.pointerId) return;
    stripDrag = null;
    app.panTo(stripTime(event), true);
  }
  const zoomLabel = $derived(view.zoom === "fit" ? "Activity" : zoomLevels.find(level => level.value === view.zoom)?.label ?? "");
</script>

<svelte:window bind:innerWidth={viewportWidth} onscroll={leave} />

<section class="scrubber card" aria-label="Timeline">
  <div class="toolbar">
    <div class="group">
      <button class="btn small icon ghost" aria-label="Zoom out" title="Zoom out (−)" disabled={view.dayLoading || zoomIndex() <= 0}
        onclick={() => app.stepZoom(-1)}><Icon name="minus" size={14} /></button>
      <label class="zoom">
        <span class="visually-hidden">Window</span>
        <select id="zoom" value={view.zoom} disabled={view.dayLoading || view.navigationBusy}
          onchange={event => event.currentTarget.value === "fit" ? void app.fitActivity() : app.zoom(event.currentTarget.value)}>
          <option value="fit">Fit activity</option>
          {#each zoomLevels as level}<option value={level.value}>{level.label}</option>{/each}
        </select>
        <span class="zoom-label" aria-hidden="true">{zoomLabel}<Icon name="chevron-down" size={12} /></span>
      </label>
      <button class="btn small icon ghost" aria-label="Zoom in" title="Zoom in (+)" disabled={view.dayLoading || zoomIndex() >= zoomLevels.length - 1}
        onclick={() => app.stepZoom(1)}><Icon name="plus" size={14} /></button>
      <button class="btn small ghost" title="Fit the day's activity" disabled={view.dayLoading || view.navigationBusy}
        onclick={() => void app.fitActivity()}><Icon name="fit" size={14} />Fit</button>
    </div>
    <div class="range num" aria-live="polite">
      {#if visible}{clockRange(visible, timezone)}{/if}
      {#if view.rangeLoading || view.navigationBusy}<span class="spinner" aria-label="Loading"></span>{/if}
    </div>
    <div class="group">
      <button class="btn small icon ghost" aria-label="Previous screenshot" title="Previous screenshot (←)" disabled={view.dayLoading}
        onclick={() => void app.stepScreenshot(-1)}><Icon name="step-back" size={14} /></button>
      <button class="btn small icon ghost" aria-label="Next screenshot" title="Next screenshot (→)" disabled={view.dayLoading}
        onclick={() => void app.stepScreenshot(1)}><Icon name="step-forward" size={14} /></button>
      <button id="now" class="btn small" disabled={view.dayLoading} onclick={() => void app.jumpNow()} title="Go to now (Home)">
        <Icon name="now" size={14} />Now</button>
    </div>
  </div>

  {#if view.day}
    <div class="strip" bind:this={strip} role="presentation" title="Drag to move the visible window"
      onpointerdown={stripDown} onpointermove={stripMove} onpointerup={stripUp} onpointercancel={stripUp}>
      {#each evidence?.intervals ?? [] as interval (interval.range.start_ms)}
        {#if interval.kind !== "observed"}
          <div class="strip-gap hatched" style:left={dayPercent(interval.range.start_ms) + "%"}
            style:width={dayPercent(interval.range.end_ms) - dayPercent(interval.range.start_ms) + "%"}></div>
        {/if}
      {/each}
      {#each sessions ?? [] as session (session.range.start_ms)}
        <div class="strip-session" style:left={dayPercent(session.range.start_ms) + "%"}
          style:width={dayPercent(session.range.end_ms) - dayPercent(session.range.start_ms) + "%"}
          style:background={paint.session(session).swatch.fill}></div>
      {/each}
      {#if visible}
        <div class="strip-window" style:left={dayPercent(visible.start_ms) + "%"}
          style:width={Math.max(0.6, dayPercent(visible.end_ms) - dayPercent(visible.start_ms)) + "%"}></div>
      {/if}
    </div>
  {/if}

  <div id="timeline" class="tracks" class:dragging class:loading={view.rangeLoading} bind:this={tracks} bind:clientWidth={width}
    tabindex="0" role="slider" aria-label="Timeline: select a moment"
    aria-valuemin={visible?.start_ms ?? 0} aria-valuemax={visible?.end_ms ?? 0} aria-valuenow={view.selected}
    aria-valuetext={time(view.selected, timezone)} aria-busy={view.rangeLoading}
    onpointermove={move} onpointerleave={leave} onpointerdown={down} onpointerup={end} onpointercancel={end}
    onlostpointercapture={end} onclick={click} onkeydown={key} use:wheelEvents>
    {#if visible}
      {#each ticks as tick (tick)}<div class="gridline" style:left={percent(tick) + "%"}></div>{/each}
      <div class="band">
        {#each gaps as row, index (index)}
          <div class={row.focus?.kind === "no_focus" && !row.has_gap ? "no-focus" : "gap hatched"} style={place(row.range!)}></div>
        {/each}
        {#each segments as segment (segment.range.start_ms + segment.key)}
          <div class="segment" class:mixed={segment.mixed} style={place(segment.range)} style:background={segment.fill} style:color={segment.ink}>
            {#if px(segment.range) > 64}<span class="ellipsis">{segment.label}</span>{/if}
          </div>
        {/each}
        {#each idle as row, index (index)}<div class="idle" style={place(row.range!)}></div>{/each}
      </div>
      {#each shotBands as track (track.monitor)}
        <div class="shots" title={"Screenshots · " + track.monitor}>
          {#each track.bands as row, index (index)}
            <div class={row.unavailable ? "shot-gap" : "shot"} style={place(row.range!)}></div>
          {/each}
        </div>
      {/each}
      {#if showCommits}
        <div class="commits" id="commit-track" bind:this={commitTrack} aria-label={commitList ? commitCount(commitList.commits.length) + " in view" : "Commits unavailable"}>
          {#each clusters as cluster (cluster.key)}
            {@const many = cluster.commits.length > 1}
            <div class="commit" class:many class:active={hoverCluster?.key === cluster.key} style:left={percent(cluster.start_ms) + "%"}
              style:width={many ? `calc(${percent(cluster.end_ms) - percent(cluster.start_ms)}% + 14px)` : undefined}>
              {#if many}<span>{cluster.commits.length}</span>{/if}
            </div>
          {/each}
        </div>
      {/if}
      {#if view.highlight && view.highlight.end_ms > visible.start_ms && view.highlight.start_ms < visible.end_ms}
        <div id="stretch-highlight" class="highlight" style={place(view.highlight)} aria-hidden="true"></div>
      {/if}
      {#if inView(now)}<div class="now" style:left={percent(now) + "%"} title={"Now · " + clock(now, timezone)}></div>{/if}
      {#if hoverTime !== null && inView(hoverTime) && !dragging}<div class="hover-line" class:snap={!!hoverCluster} style:left={percent(hoverTime) + "%"}></div>{/if}
      <div id="cursor" class="playhead" hidden={!inView(view.selected)} style:left={percent(view.selected) + "%"}>
        <span class="num">{clock(view.selected, timezone)}</span>
      </div>
    {/if}
  </div>
  <div class="axis num" aria-hidden="true">
    {#each ticks as tick (tick)}<span style:left={percent(tick) + "%"}>{tickLabel(tick)}</span>{/each}
  </div>
</section>

{#if hoverTime !== null && visible && inView(hoverTime) && !dragging}
  <div id="timeline-tooltip" class="tooltip hover-card" class:below role="tooltip"
    style:left={cardX + "px"} style:top={(below ? trackBottom + 8 : trackTop - 12) + "px"}
    style:width={cardWidth + "px"} style:height={CARD_HEIGHT + "px"}>
    <div class="card-head">
      <strong class="num">{time(hoverTime, timezone)}</strong>
      <span class="muted num ellipsis">{hoverSegment && hoverSegment.range.end_ms - hoverSegment.range.start_ms >= 60_000
        ? `${hoverSegment.label} · ${duration(hoverSegment.range.end_ms - hoverSegment.range.start_ms)}` : ""}</span>
    </div>
    <div class="preview" class:waiting={!previewReady}>
      {#each previewFrames.slice(0, 3) as frame (frame.shot.screenshot.monitor)}
        {#if frame.draw}
          <div class="frame"><ScreenshotCanvas {frame} maxWidth={640} label={frame.shot.screenshot.monitor + " at " + time(frame.shot.screenshot.unix_ms, timezone)}
            transform={imageTransform(frame.shot.screenshot.transform)} /></div>
        {/if}
      {:else}
        <div class="no-preview">{previewReady ? (view.hoverEvidence.expired ? "Screenshot expired" : "No screenshot") : ""}</div>
      {/each}
      {#if hoverCluster}
        <ul class="commit-list">
          {#each hoverCluster.commits.slice(0, 2) as commit (commit.repository + commit.hash)}
            <li><Icon name="commit" size={13} /><span class="ellipsis"><strong>{commit.subject}</strong>
              <small>{repositoryLabel(commit.repository)} · {time(commit.time_ms, timezone)}</small></span></li>
          {/each}
          {#if hoverCluster.commits.length > 2}<li class="more">and {hoverCluster.commits.length - 2} more</li>{/if}
        </ul>
      {/if}
    </div>
    <div class="focus-line">
      {#if exact?.kind === "focus" && exact.window}
        {@const p = paint.focus(exact.window.application, exact.category ?? null)}
        <span class="dot" style:background={p.swatch.fill}></span><strong class="ellipsis">{exact.window.application}</strong>
        {#if exact.idle}<span class="badge">idle</span>{/if}{#if paint.mode === "category"}<span class="muted ellipsis">{p.detail}</span>{/if}
      {:else if hoverBlock && !hoverBlock.mixed}
        <span class="dot" style:background={hoverSegment!.fill}></span><strong class="ellipsis">{hoverBlock.group.application}</strong>
      {/if}
    </div>
    <p class="title">{exact?.kind === "focus" && exact.window ? exact.window.title
      : hoverBlock && !hoverBlock.mixed ? hoverBlock.group.title
      : hoverBlock ? "Several windows · zoom in for detail" : describeGap(hoverGap)}</p>
  </div>
{/if}

<style>
  .scrubber { padding: 10px 14px 6px; user-select: none; }
  .toolbar { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 8px; }
  @media (max-width: 700px) { .toolbar { flex-wrap: wrap; gap: 6px; } .range { order: 3; width: 100%; } }
  .group { display: flex; align-items: center; gap: 4px; }
  .range { display: flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--muted); }
  .zoom { position: relative; display: inline-flex; }
  .zoom select { position: absolute; inset: 0; opacity: 0; cursor: pointer; }
  .zoom-label {
    display: inline-flex; align-items: center; gap: 4px; height: 26px; padding: 0 8px; border-radius: 7px;
    font-size: 12px; font-weight: 500; color: var(--text-2); background: var(--surface-2); border: 1px solid var(--border);
    min-width: 92px; justify-content: space-between;
  }
  .zoom:focus-within .zoom-label { outline: 2px solid var(--accent); outline-offset: 1px; }

  .strip {
    position: relative; height: 14px; margin-bottom: 8px; border-radius: 4px; background: var(--sunken);
    cursor: grab; overflow: hidden;
  }
  .strip-gap, .strip-session { position: absolute; top: 0; bottom: 0; }
  .strip-session { top: 3px; bottom: 3px; border-radius: 1px; opacity: 0.85; }
  .strip-window {
    position: absolute; top: 0; bottom: 0; border-radius: 4px;
    background: color-mix(in oklab, var(--text), transparent 88%); box-shadow: inset 0 0 0 1.5px var(--text-2);
  }

  .tracks {
    position: relative; display: flex; flex-direction: column; gap: 4px; padding: 18px 0 4px;
    cursor: crosshair; touch-action: pan-y; outline-offset: 4px; border-radius: 6px;
  }
  .tracks.dragging { cursor: grabbing; }
  .tracks.loading .band, .tracks.loading .shots { opacity: 0.6; transition: opacity 0.2s 0.15s; }
  .gridline { position: absolute; top: 14px; bottom: 0; width: 1px; background: var(--border); pointer-events: none; }
  .band {
    position: relative; height: 44px; border-radius: 6px; background: var(--sunken); overflow: hidden;
  }
  .band > div { position: absolute; top: 0; bottom: 0; }
  .segment {
    display: flex; align-items: center; padding: 0 7px; font-size: 11.5px; font-weight: 600;
    border-left: 1px solid var(--sunken); min-width: 1px;
  }
  .segment.mixed { background-image: repeating-linear-gradient(135deg, rgb(255 255 255 / 0.22) 0 2px, transparent 2px 6px); }
  .no-focus { background: color-mix(in oklab, var(--text), transparent 94%); }
  .idle {
    background: repeating-linear-gradient(135deg, color-mix(in oklab, var(--sunken), transparent 25%) 0 3px, transparent 3px 6px);
    pointer-events: none;
  }
  .shots { position: relative; height: 4px; border-radius: 2px; background: var(--sunken); overflow: hidden; }
  .shots > div { position: absolute; top: 0; bottom: 0; }
  .shot { background: color-mix(in oklab, var(--text-2), transparent 55%); }
  .shot-gap { background: color-mix(in oklab, var(--bad), transparent 50%); }
  .commits { position: relative; height: 16px; }
  .tracks:has(.commit.active) { cursor: pointer; }
  .commit {
    position: absolute; top: 3px; height: 10px; width: 10px; margin-left: -5px; border-radius: 5px;
    background: var(--text); box-shadow: 0 0 0 2px var(--surface); pointer-events: none;
  }
  .commit.many {
    top: 1px; height: 14px; margin-left: -7px; min-width: 14px; border-radius: 7px;
    display: flex; align-items: center; justify-content: center; font-size: 9.5px; font-weight: 700; color: var(--surface);
  }
  .commit { transition: transform 0.12s, background 0.12s, box-shadow 0.12s; }
  .commit.active {
    background: var(--accent-strong); color: #fff; transform: scale(1.3); z-index: 2;
    box-shadow: 0 0 0 2px var(--surface), 0 0 0 5px var(--accent-soft);
  }
  .hover-line.snap { width: 0; border-left: 1.5px dashed var(--accent-strong); background: none; }
  .highlight {
    position: absolute; top: 14px; bottom: 0; border-radius: 6px; pointer-events: none;
    background: var(--accent-soft); box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  .now { position: absolute; top: 14px; bottom: 0; width: 2px; margin-left: -1px; background: var(--bad); pointer-events: none; }
  .hover-line { position: absolute; top: 14px; bottom: 0; width: 1px; background: var(--text-2); pointer-events: none; }
  .playhead { position: absolute; top: 14px; bottom: 0; width: 2px; margin-left: -1px; background: var(--accent-strong); pointer-events: none; }
  .playhead span {
    position: absolute; top: -16px; left: 50%; transform: translateX(-50%); padding: 1px 6px; border-radius: 5px;
    background: var(--accent); color: #fff; font-size: 10.5px; font-weight: 600; white-space: nowrap;
  }
  .axis { position: relative; height: 20px; font-size: 11px; color: var(--muted); }
  .axis span { position: absolute; top: 3px; transform: translateX(-50%); white-space: nowrap; }

  .hover-card.below { transform: none; }
  .hover-card {
    transform: translateY(-100%); padding: 10px; display: grid; gap: 6px;
    grid-template-rows: 18px 1fr 18px 36px; overflow: hidden;
  }
  .card-head { display: flex; justify-content: space-between; align-items: baseline; gap: 8px; font-size: 12px; min-width: 0; }
  .card-head strong { font-size: 13px; }
  .preview {
    position: relative; display: flex; align-items: center; justify-content: center; gap: 4px; min-height: 0;
    border-radius: 6px; overflow: hidden; background: var(--sunken); transition: opacity 0.15s;
  }
  .preview.waiting { opacity: 0.55; }
  .frame { flex: 0 1 auto; min-width: 0; height: 100%; display: flex; align-items: center; justify-content: center; }
  .preview :global(canvas) { display: block; max-width: 100%; max-height: 100%; width: auto; height: auto; object-fit: contain; }
  .no-preview { font-size: 12px; color: var(--muted); }
  .focus-line { display: flex; align-items: center; gap: 6px; min-width: 0; }
  .title { color: var(--text-2); overflow: hidden; line-height: 18px; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; word-break: break-word; }
  .commit-list {
    position: absolute; left: 0; right: 0; bottom: 0; list-style: none; margin: 0; padding: 6px 8px;
    display: flex; flex-direction: column; gap: 4px; background: color-mix(in oklab, var(--surface-2), transparent 8%);
    border-top: 1px solid var(--border);
  }
  .commit-list .more { color: var(--muted); padding-left: 19px; }
  .commit-list li { display: flex; gap: 6px; align-items: flex-start; }
  .commit-list span { display: flex; flex-direction: column; }
  .commit-list small { color: var(--muted); }
</style>
