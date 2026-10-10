<script lang="ts">
  import ScreenshotCanvas from "./ScreenshotCanvas.svelte";
  import SuspenseBoundary from "./SuspenseBoundary.svelte";
  import { onMount } from "svelte";
  import { on } from "svelte/events";
  import { applicationSwatch, clock, clockRange, coverageBands, duration, noScreenshot, swatchStyle, time, imageTransform, UNCATEGORIZED, type Swatch } from "./format";
  import { hourSwitches, plural, SWITCH_LEVELS, switchLevel, switchLevelLabel } from "./metrics";
  import { zoomLevels, type TimelineApp, type View } from "./model";
  import { colorBy } from "./preferences";
  import ColorBy from "./ColorBy.svelte";
  import { categoryGrouping, categoryLabel, categoryNodes, categorySwatches, grouping, isNested } from "./categories";
  import { clusterCommits, commitCount, commitProblems, repositoryLabel, repositorySummary, type CommitCluster } from "./commits";
  import type { Block, Commit, Range } from "./types";
  let { view, app }: { view: View; app: TimelineApp } = $props();
  const blocks = $derived(view.blocks);
  const rows = $derived(view.rows);
  const visible = $derived(view.visible);
  const timezone = $derived(view.timezone);
  let expanded = $state(false);
  // Lanes and fills follow the application, the top-level category, or the
  // most specific category.
  const nodes = $derived(categoryNodes(view.categories));
  const configured = $derived(nodes.length > 0);
  const choice = $derived(grouping($colorBy, nodes));
  const mode = $derived(choice === "application" ? "application" : "category");
  const shown = $derived(categoryGrouping(nodes, choice === "subcategory" ? "subcategory" : "category"));
  const categorySwatchOf = $derived(categorySwatches(nodes));
  const blockSwatch = (block: Block): Swatch => mode === "category"
    ? categorySwatchOf(shown.at(block.category)) : applicationSwatch(block.group.application);
  const blockLabel = (block: Block) => mode === "category" ? shown.at(block.category) ?? UNCATEGORIZED : block.group.application;
  /** Categories shown for the visible blocks, in tree order. */
  const legend = $derived.by(() => {
    const present = new Set(blocks.map(block => shown.at(block.category)));
    return view.categories.filter(row => present.has(row.category));
  });
  const groupName = $derived(choice === "subcategory" ? "Subcategory" : mode === "category" ? "Category" : "Application");
  // Hourly switches of the loaded range, following the color mode.
  const hours = $derived(view.metrics?.hours ?? []);
  const switchNoun = $derived(mode === "category" ? "category switch" : "application switch");
  const describeHour = (hour: (typeof hours)[number]) => `${clockRange(hour.range, timezone)} · `
    + `${plural(hourSwitches(hour, mode), switchNoun, switchNoun + "es")} in ${duration(hour.active_ms)} active`;
  let timelineWidth = $state(0);
  $effect(() => app.setWidth(timelineWidth));
  // Commits: markers clustered where they would overlap at the current scale.
  const commitList = $derived(view.commits);
  const showCommits = $derived(!!commitList?.configured || !!view.commitError);
  const clusters = $derived(visible && commitList ? clusterCommits(commitList.commits, visible, timelineWidth) : []);
  const commitsInView = $derived(clusters.reduce((sum, cluster) => sum + cluster.commits.length, 0));
  const commitProblem = $derived(view.commitError ? "Commits unavailable: " + view.commitError : commitProblems(commitList));
  let commitFocus = $state(0);
  const commitTab = $derived(Math.min(commitFocus, Math.max(0, clusters.length - 1)));
  const commitButtons: HTMLButtonElement[] = [];
  let keyboardCluster = $state<CommitCluster | null>(null);
  const clusterTime = (cluster: CommitCluster) => Math.round((cluster.start_ms + cluster.end_ms) / 2);
  /** The marker under the hover line, within its drawn extent. */
  const hoverCluster = $derived.by(() => {
    if (keyboardCluster) return keyboardCluster;
    if (hoverTime === null || !visible || timelineWidth <= 0) return undefined;
    const slack = 7 * (visible.end_ms - visible.start_ms) / timelineWidth;
    return clusters.find(cluster => cluster.start_ms - slack <= hoverTime! && hoverTime! <= cluster.end_ms + slack);
  });
  const describeCommit = (commit: Commit) => `${repositoryLabel(commit.repository)}: ${commit.subject} (${commit.short_hash}, ${time(commit.time_ms, timezone)})`;
  const describeCluster = (cluster: CommitCluster) => (cluster.commits.length === 1 ? "Commit " : commitCount(cluster.commits.length) + ": ")
    + cluster.commits.slice(0, 3).map(describeCommit).join("; ") + (cluster.commits.length > 3 ? `; and ${cluster.commits.length - 3} more` : "")
    + ". Enter selects this moment.";
  /** Shows a focused marker's commits in the tooltip, like hovering it. */
  function focusCommit(index: number, element: HTMLElement) {
    commitFocus = index;
    const cluster = clusters[index];
    const rect = element.getBoundingClientRect();
    keyboardCluster = cluster;
    pointerX = rect.left + rect.width / 2;
    pointerY = rect.bottom;
    hoverTime = clusterTime(cluster);
    app.hover(hoverTime);
  }
  function blurCommit() {
    if (!keyboardCluster) return;
    keyboardCluster = null;
    leave();
  }
  function selectCommit(event: MouseEvent, cluster: CommitCluster) {
    event.stopPropagation();
    if (!view.dayLoading) void app.select(cluster.commits[0].time_ms);
  }
  function commitKey(event: KeyboardEvent, index: number) {
    // Arrows move between markers rather than stepping screenshots.
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      event.stopPropagation();
      const next = Math.max(0, Math.min(clusters.length - 1, index + (event.key === "ArrowRight" ? 1 : -1)));
      commitButtons[next]?.focus();
    } else if (event.key === "Escape") {
      event.stopPropagation();
      blurCommit();
    } else if (event.key === "Enter" || event.key === " ") event.stopPropagation();
  }
  /** Opens and shows the repository details at the end of the page. */
  function showRepositories(event: MouseEvent) {
    event.stopPropagation();
    const details = document.getElementById("repositories") as HTMLDetailsElement | null;
    if (!details) return;
    details.open = true;
    details.scrollIntoView({ behavior: "smooth", block: "center" });
    details.querySelector("summary")?.focus({ preventScroll: true });
  }
  let hoverTime = $state<number | null>(null);
  const hoverHour = $derived(hoverTime === null ? undefined
    : hours.find(hour => hour.range.start_ms <= hoverTime! && hoverTime! < hour.range.end_ms));
  let pointerX = $state(0), pointerY = $state(0);
  let viewportWidth = $state(0), viewportHeight = $state(0);
  let popupWidth = $state(320), popupHeight = $state(200);
  const popupX = $derived(Math.max(8, Math.min(pointerX + 16 + popupWidth > viewportWidth
    ? pointerX - popupWidth - 16 : pointerX + 16, viewportWidth - popupWidth - 8)));
  const popupY = $derived(Math.max(8, Math.min(pointerY + 16 + popupHeight > viewportHeight
    ? pointerY - popupHeight - 16 : pointerY + 16, viewportHeight - popupHeight - 8)));
  const focusBands = $derived(coverageBands(rows.filter(r => r.kind === "focus_coverage")));
  const hoverBlock = $derived(blocks.find(block => hoverTime !== null && block.range.start_ms <= hoverTime && hoverTime < block.range.end_ms));
  const hoverFocus = $derived(focusBands.find(row => hoverTime !== null
    && row.range && row.range.start_ms <= hoverTime && hoverTime < row.range.end_ms)?.focus);
  const previewFrames = $derived(view.hoverEvidence.frames ?? view.frames);
  const previewReady = $derived(hoverTime !== null && view.hoverEvidence.time === hoverTime);
  const exactFocus = $derived(previewReady ? view.hoverEvidence.focus.find(row => row.focus)?.focus : undefined);
  let now = $state(Date.now());
  let lastWheelZoom = -Infinity;
  let dragging = $state(false);
  let gesture: { id: number; x: number; width: number; range: Range; date: string; zoom: string } | null = null;
  let suppressClick = false;
  let panFrame = 0, pendingPan: number | null = null;
  function cancelPanFrame() {
    cancelAnimationFrame(panFrame); panFrame = 0; pendingPan = null;
  }
  function schedulePan(center: number) {
    pendingPan = center;
    if (!panFrame) panFrame = requestAnimationFrame(() => {
      panFrame = 0;
      const next = pendingPan; pendingPan = null;
      if (next !== null && gesture?.date === view.date && gesture.zoom === view.zoom) app.panTo(next);
    });
  }
  const imageTracks = $derived(Array.from(new Set(rows.filter(r => r.kind === "image_coverage").map(r => r.monitor!)),
    monitor => ({ monitor, bands: coverageBands(rows.filter(r => r.kind === "image_coverage" && r.monitor === monitor)) })));
  const previewHeight = $derived.by(() => {
    const shots = previewFrames?.length ? previewFrames.map(frame => frame.shot) : view.shots;
    const count = Math.min(2, Math.max(1, imageTracks.length, shots.length));
    const width = Math.max(1, popupWidth - 24);
    let height = (count - 1) * 8;
    for (let i = 0; i < count; i++) {
      const shot = shots[i]?.screenshot;
      height += width * (shot ? shot.height / shot.width : 9 / 16) + 18;
    }
    return height;
  });
  const lanes = $derived(mode === "category"
    ? legend.map(row => ({ key: "category:" + (row.category ?? ""), label: row.category ?? UNCATEGORIZED,
      blocks: blocks.filter(block => shown.at(block.category) === row.category) }))
    : Array.from(new Set(blocks.map(block => block.group.application)), application => ({
      key: "application:" + application, label: application,
      blocks: blocks.filter(block => block.group.application === application) })));
  onMount(() => {
    const timer = setInterval(() => now = Date.now(), 1000);
    return () => { clearInterval(timer); cancelPanFrame(); };
  });
  // Seconds matter only when ticks are minutes apart.
  const tickLabel = (ms: number) => visible && visible.end_ms - visible.start_ms >= 20 * 60_000 ? clock(ms, timezone) : time(ms, timezone);
  const percent = (ms: number) => visible ? 100 * (ms - visible.start_ms) / (visible.end_ms - visible.start_ms) : 0;
  const inView = (ms: number) => !!visible && ms >= visible.start_ms && ms < visible.end_ms;
  const position = (range: Range) => "left:" + percent(range.start_ms) + "%;width:" + (percent(range.end_ms) - percent(range.start_ms)) + "%";
  function anchor(event: MouseEvent) {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    return Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width));
  }
  function pointerTime(event: MouseEvent) {
    const range = visible!;
    return Math.round(Math.min(range.end_ms - 1, range.start_ms + anchor(event) * (range.end_ms - range.start_ms)));
  }
  function move(event: PointerEvent) {
    if (!visible || view.dayLoading) return;
    if (gesture && event.pointerId === gesture.id) {
      if (gesture.date !== view.date || gesture.zoom !== view.zoom) { end(event); return; }
      const distance = event.clientX - gesture.x;
      if (Math.abs(distance) >= 4) dragging = true;
      if (dragging) {
        hoverTime = null;
        const range = gesture.range;
        schedulePan((range.start_ms + range.end_ms) / 2
          - distance / gesture.width * (range.end_ms - range.start_ms));
      }
      return;
    }
    keyboardCluster = null;
    pointerX = event.clientX;
    pointerY = event.clientY;
    hoverTime = pointerTime(event);
    // Position follows every event; the model debounces evidence requests.
    app.hover(hoverTime);
  }
  function leave() { hoverTime = null; app.leave(); }
  function down(event: PointerEvent) {
    if (!visible || view.dayLoading || event.button !== 0 || gesture) return;
    const element = event.currentTarget as HTMLElement;
    gesture = { id: event.pointerId, x: event.clientX, width: element.getBoundingClientRect().width,
      range: { ...visible }, date: view.date, zoom: view.zoom };
    suppressClick = false;
    leave();
    element.setPointerCapture(event.pointerId);
  }
  function end(event: PointerEvent) {
    if (!gesture || gesture.id !== event.pointerId) return;
    suppressClick = dragging || event.type === "pointercancel";
    const sameRange = gesture.date === view.date && gesture.zoom === view.zoom;
    const center = pendingPan ?? (visible ? (visible.start_ms + visible.end_ms) / 2 : null);
    cancelPanFrame();
    gesture = null;
    if (dragging && sameRange && center !== null) app.panTo(center, true);
    dragging = false;
    const element = event.currentTarget as HTMLElement;
    if (element.hasPointerCapture(event.pointerId)) element.releasePointerCapture(event.pointerId);
  }
  function click(event: MouseEvent) {
    if (suppressClick) { suppressClick = false; return; }
    if (visible && !view.dayLoading) void app.select(pointerTime(event));
  }
  function key(event: KeyboardEvent) {
    if (event.key === "Escape") { leave(); return; }
    if (!visible || view.dayLoading) return;
    if (["ArrowLeft", "ArrowRight"].includes(event.key)) {
      event.preventDefault();
      const direction = event.key === "ArrowRight" ? 1 : -1;
      if (event.shiftKey) app.pan(direction); else void app.stepScreenshot(direction);
    } else if (["+", "=", "-"].includes(event.key)) {
      event.preventDefault(); app.stepZoom(event.key === "-" ? -1 : 1);
    } else if (event.key === "Home") { event.preventDefault(); void app.jumpNow(); }
  }
  function wheel(event: WheelEvent) {
    if (gesture || event.ctrlKey || !event.deltaY || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
    event.preventDefault();
    if (!visible || view.dayLoading || performance.now() - lastWheelZoom < 180) return;
    const index = view.zoom === "fit" ? Math.max(0, zoomLevels.findIndex(level =>
      (level.durationMs ?? 86_400_000 / Number(level.value)) <= view.fitWidth!)) : zoomLevels.findIndex(level => level.value === view.zoom);
    const next = Math.max(0, Math.min(zoomLevels.length - 1, index + (event.deltaY < 0 ? 1 : -1)));
    if (next === index) return;
    const fraction = anchor(event);
    const ms = Math.round(visible.start_ms + fraction * (visible.end_ms - visible.start_ms));
    lastWheelZoom = performance.now();
    app.zoom(zoomLevels[next].value, fraction, ms, true);
    pointerX = event.clientX;
    pointerY = event.clientY;
    hoverTime = ms;
    app.hover(ms);
  }
  function wheelEvents(element: HTMLElement) {
    return { destroy: on(element, "wheel", wheel, { passive: false }) };
  }
</script>

<svelte:window bind:innerWidth={viewportWidth} bind:innerHeight={viewportHeight} onscroll={leave} />

{#snippet categoryTag(category: string | null)}
  <span class="category-tag"><span class="swatch" style={swatchStyle(categorySwatchOf(category))}></span>{categoryLabel(nodes, category)}</span>
{/snippet}

{#snippet blockTrack(label: string, blocks: Block[])}
  <div class="track-label">{label}</div>
  <div class="track">
    {#each blocks as block (block.id)}
      <div class={"block" + (block.mixed ? " mixed" : "")} style={position(block.range) + ";" + swatchStyle(blockSwatch(block))}
        data-description={block.group.application + " · " + block.group.title + " · " + categoryLabel(nodes, block.category) + " · " + time(block.range.start_ms, timezone) + " – " + time(block.range.end_ms, timezone) + " · " + block.id}>
        {blockLabel(block)}
      </div>
    {/each}
  </div>
{/snippet}

<nav class="timeline-navigation" aria-label="Timeline navigation">
  <div class="navigation-row">
    <div class="control-group date-controls">
      <button id="previous" aria-label="Previous day" disabled={!view.day} onclick={() => app.shiftDate(-1)}>‹</button>
      <label class="date-label">Activity day
        <input type="date" id="date" aria-label="Activity date" value={view.date} onchange={event => void app.loadDay(event.currentTarget.value)} />
      </label>
      <button id="next" aria-label="Next day" disabled={!view.day} onclick={() => app.shiftDate(1)}>›</button>
    </div>
    <button id="now" class="primary-action" disabled={view.dayLoading} onclick={() => void app.jumpNow()}>Now</button>
    <div class="control-group zoom-controls">
      <label>Window
        <select id="zoom" value={view.zoom} disabled={view.dayLoading || view.navigationBusy}
          onchange={event => event.currentTarget.value === "fit" ? void app.fitActivity() : app.zoom(event.currentTarget.value)}>
          <option value="fit">Fit activity</option>
          {#each zoomLevels as level}<option value={level.value}>{level.label}</option>{/each}
        </select>
      </label>
    </div>
    {#if configured}<ColorBy nested={isNested(nodes)} />{/if}
    <button id="lanes" class="lanes-toggle" aria-expanded={expanded} onclick={() => expanded = !expanded}>{expanded ? "▾" : "▸"} {groupName} lanes</button>
  </div>
</nav>
<div id="timeline" bind:clientWidth={timelineWidth} tabindex="0" role="slider" aria-label="Select time"
  aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.max(0, Math.min(100, percent(view.selected)))}
  aria-valuetext={time(view.selected, timezone)} aria-busy={view.rangeLoading}
  aria-describedby={hoverTime !== null ? "timeline-tooltip" : undefined}
  class:dragging onpointermove={move} onpointerleave={leave} onpointerdown={down}
  onpointerup={end} onpointercancel={end} onlostpointercapture={end} onclick={click} onkeydown={key} use:wheelEvents>
  {#if visible}
    {#if mode === "category"}
      <ul class="legend" id="category-legend" aria-label="Categories">
        {#each legend as row (row.category)}
          <li><span class="swatch" style={swatchStyle(categorySwatchOf(row.category))}></span>{row.category ?? UNCATEGORIZED}</li>
        {:else}<li>No focus in view</li>{/each}
      </ul>
    {/if}
    {@render blockTrack(`Observed focus · by ${mode} · striped = mixed activity`, blocks)}
    <div class="track-label">Focus coverage · striped = contains unknown/gap · gold = contains idle</div>
    <div class="track">
      {#each focusBands as row}
        {#if row.focus?.kind !== "focus" || row.has_gap}
          <div class={"block " + (row.focus?.kind === "no_focus" && !row.has_gap ? "no-focus" : "gap")}
            style={position(row.range!)} data-description={row.has_gap && row.mixed ? "Mixed focus coverage · zoom in" : row.focus?.reason || row.focus?.kind || ""}>
            {(row.has_gap && row.mixed ? "Mixed focus coverage · zoom in" : row.focus?.reason) || (row.focus?.kind === "no_focus" ? "No focused window" : row.focus?.kind) || ""}
          </div>
        {/if}
        {#if row.idle || row.focus?.idle}<div class="block idle" style={position(row.range!)} data-description="Idle"></div>{/if}
      {/each}
    </div>
    {#if hours.length}
      <div class="track-label switch-label">
        <span>{mode === "category" ? "Category" : "Application"} switches per hour</span>
        <span class="scale-legend" aria-hidden="true">
          {#each SWITCH_LEVELS as _, index}<span><span class="swatch" style:background={`var(--ramp-${index + 1})`}></span>{switchLevelLabel(index + 1)}</span>{/each}
        </span>
      </div>
      <div class="track switch-track" id="switch-strip">
        {#each hours as hour (hour.range.start_ms)}
          {@const level = switchLevel(hourSwitches(hour, mode))}
          {#if level}<div class="block switch-cell" style={position(hour.range) + `;background:var(--ramp-${level})`}
            data-description={describeHour(hour)}></div>{/if}
        {/each}
      </div>
    {/if}
    {#if showCommits}
      <div class="track-label commit-label">
        <span>Commits · {view.commitError ? "unavailable" : commitList?.commits.length ? `${commitsInView} in view` : "none in this range"}</span>
        {#if commitProblem}
          <button class="link-button commit-problem" onpointerdown={event => event.stopPropagation()} onclick={showRepositories}>{commitProblem}</button>
        {/if}
      </div>
      <div class="track commit-track" id="commit-track" role="group" aria-label="Commits · arrows move between markers, Enter selects">
        {#each clusters as cluster, index (cluster.key)}
          {@const multiple = cluster.commits.length > 1}
          <button bind:this={commitButtons[index]} class="commit-marker" class:cluster={multiple}
            style:left={percent(cluster.start_ms) + "%"} style:width={multiple ? `calc(${percent(cluster.end_ms) - percent(cluster.start_ms)}% + 16px)` : undefined}
            tabindex={index === commitTab ? 0 : -1} aria-label={describeCluster(cluster)}
            aria-describedby={keyboardCluster === cluster ? "timeline-tooltip" : undefined}
            onpointerdown={event => event.stopPropagation()} onclick={event => selectCommit(event, cluster)}
            onkeydown={event => commitKey(event, index)} onfocus={event => focusCommit(index, event.currentTarget)} onblur={blurCommit}>
            {#if multiple}<span class="commit-count">{cluster.commits.length}</span>{/if}
          </button>
        {/each}
      </div>
    {/if}
    {#each imageTracks as track (track.monitor)}
      <div class="track-label">Screenshots · {track.monitor}</div>
      <div class="track">
        {#each track.bands as row}
          <div class={"block " + (row.unavailable ? "gap" : "image-coverage")}
            style={position(row.range!)}
            data-description={row.unavailable || "Screenshot coverage"}>{row.unavailable || ""}</div>
        {/each}
      </div>
    {/each}
    {#if expanded}
      {#each lanes as lane (lane.key)}
        {@render blockTrack(lane.label, lane.blocks)}
      {/each}
    {/if}
    {#if view.highlight && view.highlight.end_ms > visible.start_ms && view.highlight.start_ms < visible.end_ms}
      <div id="stretch-highlight" class="stretch-highlight" style={position(view.highlight)} aria-hidden="true"></div>
    {/if}
    <div id="cursor" class="cursor" hidden={!inView(view.selected)} style:left={percent(view.selected) + "%"}></div>
    <div id="hover-cursor" class="hover-cursor" hidden={hoverTime === null || !inView(hoverTime)} aria-hidden="true" style:left={percent(hoverTime ?? 0) + "%"}></div>
    <div id="now-cursor" class="now-cursor" hidden={!inView(now)} role="img"
      aria-label={"Current time: " + time(now, timezone)} style:left={percent(now) + "%"}><span>Now</span></div>
    <div class="ticks">
      {#each [0, 0.25, 0.5, 0.75, 1] as fraction}
        <span>{tickLabel(visible.start_ms + fraction * (visible.end_ms - visible.start_ms))}</span>
      {/each}
    </div>
  {/if}
</div>
{#if hoverTime !== null && visible && inView(hoverTime)}
  <div id="timeline-tooltip" class="timeline-tooltip" role="tooltip"
    bind:clientWidth={popupWidth} bind:clientHeight={popupHeight}
    style:left={popupX + "px"} style:top={popupY + "px"}>
    <strong class="tooltip-time">{time(hoverTime, timezone)}</strong>
    {#if hoverCluster}
      <ul class="tooltip-commits" aria-label={commitCount(hoverCluster.commits.length)}>
        {#each hoverCluster.commits.slice(0, 4) as commit (commit.repository + commit.hash)}
          <li>
            <span class="commit-key" aria-hidden="true"></span>
            <span class="commit-text">
              <span class="commit-subject">{commit.subject}</span>
              <small>{repositoryLabel(commit.repository)}{commit.branch ? ` · ${commit.branch}` : ""} · <code>{commit.short_hash}</code> · {time(commit.time_ms, timezone)}{commit.amends ? ` · amended, first ${time(commit.first_ms!, timezone)}` : ""}</small>
            </span>
          </li>
        {/each}
        {#if hoverCluster.commits.length > 4}<li class="muted">and {hoverCluster.commits.length - 4} more · {repositorySummary(hoverCluster.commits)}</li>{/if}
      </ul>
    {/if}
    <div class="tooltip-focus">
    {#if exactFocus?.kind === "focus"}
      <strong>{exactFocus.window!.application}{exactFocus.idle ? " · idle" : ""}</strong>
      <p class="tooltip-title">{exactFocus.window!.title}</p>
      {#if configured}{@render categoryTag(exactFocus.category ?? null)}{/if}
    {:else if exactFocus}
      <p class="tooltip-title">{exactFocus.reason || exactFocus.kind}</p>
    {:else if hoverBlock}
      <strong>{hoverBlock.mixed ? "Mixed activity" : hoverBlock.group.application}{hoverFocus?.idle ? " · idle" : ""}</strong>
      <p class="tooltip-title">{hoverBlock.group.title}</p>
      {#if configured && !hoverBlock.mixed}{@render categoryTag(hoverBlock.category)}{/if}
    {:else}
      <p class="tooltip-title">{hoverFocus?.reason || (hoverFocus?.kind === "no_focus" ? "No focused window" : "No focus evidence at this time.")}</p>
    {/if}
    </div>
    {#if hoverHour}<p class="tooltip-switches">{describeHour(hoverHour)}</p>{/if}
    <div class="tooltip-preview" aria-busy={!previewReady}
      style:height={previewHeight + "px"}>
      <SuspenseBoundary ready={previewFrames !== null}>
        {#snippet pending()}{/snippet}
        {#if view.hoverEvidence.error}<p class="error">{view.hoverEvidence.error}</p>
        {:else}
          {#each previewFrames ?? [] as frame (frame.shot.screenshot.monitor)}
            <div class="tooltip-monitor">
              {#if frame.draw}
                <ScreenshotCanvas {frame} maxWidth={640} label={frame.shot.screenshot.monitor + " at " + time(frame.shot.screenshot.unix_ms, timezone)}
                  transform={imageTransform(frame.shot.screenshot.transform)} />
              {:else}<p class="error">{frame.error}</p>{/if}
              <span>{frame.shot.screenshot.monitor} · {time(frame.shot.screenshot.unix_ms, timezone)}{frame.shot.stale ? " · stale" : ""}</span>
            </div>
          {:else}<p>{noScreenshot(previewReady ? view.hoverEvidence.expired : view.shotsExpired)}</p>{/each}
        {/if}
      </SuspenseBoundary>
    </div>
    <span class="muted">Click to select · drag to pan · scroll to zoom</span>
  </div>
{/if}
<p id="hover" class="muted">
  Hover to preview · click to select · drag to pan · scroll to zoom.
  <span class="keyboard-hints">← / → screenshots · Shift + arrows pan · + / − zoom · Home goes to now.</span>
</p>
