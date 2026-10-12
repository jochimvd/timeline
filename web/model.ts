import { writable, readonly } from "svelte/store";
import { query, pages } from "./api";
import type { Session } from "./session";
import { activityDate as activityDateIn, addDays } from "./dates";
import { createImageLoader, createImagePool, type ImageFrame } from "./images";
import type { Range, Block, CategoryRow, CommitList, Coverage, FocusMetrics, Overview, Shot, Evidence, OverviewUpdates } from "./types";

export const zoomLevels: { value: string; label: string; durationMs?: number }[] = [
  { value: "1", label: "Full day" }, { value: "2", label: "12 hours" },
  { value: "4", label: "6 hours" }, { value: "12", label: "2 hours" },
  { value: "24", label: "1 hour", durationMs: 3_600_000 },
  { value: "48", label: "30 minutes", durationMs: 1_800_000 },
  { value: "96", label: "15 minutes", durationMs: 900_000 },
  { value: "288", label: "5 minutes", durationMs: 300_000 },
  { value: "1440", label: "1 minute", durationMs: 60_000 },
];

export type View = {
  date: string; timezone: string; settingsPath: string | null; zoom: string; layout: string; fitWidth?: number; navigationBusy: boolean;
  day?: Range; visible?: Range; loadedRange?: Range; selected: number;
  dayLoading: boolean; rangeLoading: boolean; evidenceLoading: boolean;
  blocks: Block[]; rows: Coverage[]; totals?: Overview["totals"];
  /** Context switches and focus stretches of the loaded range. */
  metrics?: FocusMetrics;
  /** A range the user asked to see on the timeline, such as the longest stretch. */
  highlight: Range | null;
  /** Configured categories in tree order, then uncategorized focus. */
  categories: CategoryRow[];
  /** The user's git commits in the loaded range, swapped together with its rows. */
  commits: CommitList | null;
  /** Why commits could not be read; the timeline remains usable. */
  commitError: string;
  focus: Coverage[]; shots: Shot[]; frames: ImageFrame[] | null; shotsExpired: boolean;
  evidenceTime: number | null;
  hoverEvidence: { time: number | null; focus: Coverage[]; frames: ImageFrame[] | null; expired: boolean; error: string; loading: boolean };
  evidenceError: string; message: string;
  /** The last step's landing time and what it passed over as unchanged. */
  skip: { at: number; count: number; ms: number } | null;
};

/** All asynchronous transitions publish complete view state; views never manage requests. */
export function createTimelineApp() {
  let view: View = {
    date: "", timezone: Intl.DateTimeFormat().resolvedOptions().timeZone, settingsPath: null, zoom: "1", layout: "grid",
    navigationBusy: false, selected: 0, dayLoading: true, rangeLoading: false, highlight: null,
    evidenceLoading: false, blocks: [], rows: [], categories: [], commits: null, commitError: "", focus: [], shots: [], frames: null, shotsExpired: false,
    hoverEvidence: { time: null, focus: [], frames: null, expired: false, error: "", loading: false },
    evidenceTime: null, evidenceError: "", message: "", skip: null,
  };
  const store = writable(view);
  const patch = (values: Partial<View>) => { view = { ...view, ...values }; store.set(view); };
  const imagePool = createImagePool();
  const images = createImageLoader(imagePool);
  const hoverImages = createImageLoader(imagePool, true);
  let hoverAbort = new AbortController(), hoverGeneration = 0;
  let hoverActive = false, hoverPending = false;
  let displayedLayout = view.layout;
  let rangeGeneration = 0, selectionGeneration = 0;
  let rangeAbort = new AbortController(), selectionAbort = new AbortController();
  const lifetime = new AbortController();
  /** Whether the day view is shown; hidden views do not poll for live updates. */
  let active = true, attached = false;
  let navigationAbort = new AbortController();
  let refreshTimer: ReturnType<typeof setInterval> | undefined;
  let hoverTimer: ReturnType<typeof setTimeout> | undefined;
  let rangeTimer: ReturnType<typeof setTimeout> | undefined;
  let hoverTime = 0;
  let disposed = false;
  let displayPoints = 640;
  let revision = "";
  let loadedDate = "";
  let interactionGeneration = 0;
  let rawRows: Coverage[] = [];
  let refreshing = false;
  let commitsRead = 0;
  // Only calendar metadata is cached; activity still comes from a fresh snapshot.
  const resolvedDays = new Map<string, Overview["resolved"]>();

  function failure(error: unknown) {
    if (!disposed && !(error instanceof DOMException && error.name === "AbortError"))
      patch({ message: error instanceof Error ? error.message : String(error) });
  }
  function cancelNavigation() {
    navigationAbort.abort();
    if (view.navigationBusy) patch({ navigationBusy: false });
  }
  function cancelHover() { clearTimeout(hoverTimer); hoverTimer = undefined; }
  function cancelRangeTimer() { clearTimeout(rangeTimer); rangeTimer = undefined; }
  function cancelSelection() {
    selectionGeneration++;
    selectionAbort.abort();
    selectionAbort = new AbortController();
  }
  function windowWidth(day: Range, zoom = view.zoom) {
    return zoom === "fit" ? view.fitWidth! : zoomLevels.find(level => level.value === zoom)?.durationMs
      ?? (day.end_ms - day.start_ms) / Number(zoom);
  }
  /** Recorded activity with a margin, within the day and at least a minute. */
  const fittedWidth = (day: Range, activity: Range) =>
    Math.min(day.end_ms - day.start_ms, Math.max(60_000, Math.ceil((activity.end_ms - activity.start_ms) * 1.1)));
  function setVisible(center: number, anchor = 0.5, bounded = false) {
    if (!view.day) return;
    const width = windowWidth(view.day);
    const start = Math.round(bounded ? Math.max(view.day.start_ms, Math.min(view.day.end_ms - width, center - width * anchor)) : center - width * anchor);
    patch({ visible: { start_ms: start, end_ms: start + width } });
  }

  type Summary = Pick<Overview, "totals" | "metrics">;
  function publishRows(raw: Coverage[], { totals, metrics }: Summary, token: string, range: Range, transition: Partial<View> = {}) {
    rawRows = raw;
    revision = token;
    const rows = raw
      .map(row => ({ ...row, kind: row.kind === "display_focus" ? "focus_coverage"
        : row.kind === "display_image" ? "image_coverage" : row.kind }));
    const blocks: Block[] = [];
    let previousCoverage: Coverage | undefined;
    for (const row of rows) {
      if (row.kind !== "focus_coverage" || !row.range) continue;
      if (row.focus?.kind !== "focus") { previousCoverage = undefined; continue; }
      const window = row.focus.window!;
      const group = { application: window.application, title: row.mixed ? "Mixed activity · zoom in for detail" : window.title };
      const category = row.focus.category ?? null;
      const previous = blocks.at(-1);
      if (previous && previousCoverage?.segment === row.segment && previousCoverage?.recording_boundary === row.recording_boundary
        && previous.range.end_ms === row.range.start_ms && previous.mixed === row.mixed && previous.category === category
        && previous.group.application === group.application && previous.group.title === group.title) {
        previous.range.end_ms = row.range.end_ms;
      } else blocks.push({ id: "display:" + row.range.start_ms + ":" + blocks.length, range: { ...row.range }, group, category, mixed: row.mixed, idle_ms: 0 });
      previousCoverage = row;
    }
    // In tree order, uncategorized last; views rank them as they need.
    const categories = rows.filter(row => row.kind === "category") as CategoryRow[];
    patch({ ...transition, rows, blocks, categories, totals, metrics, loadedRange: range, rangeLoading: false,
      message: blocks.length ? "" : "No recorded focus in this range. Coverage and screenshots may differ." });
  }
  /** Commits are external evidence: a failure leaves the timeline usable and says why. */
  async function readCommits(range: Range, signal: AbortSignal): Promise<Pick<View, "commits" | "commitError">> {
    commitsRead = Date.now();
    try {
      const commits = await query<CommitList>("commits", { range }, signal);
      return { commits, commitError: "" };
    } catch (error) {
      signal.throwIfAborted();
      return { commits: null, commitError: error instanceof Error ? error.message : String(error) };
    }
  }
  /** One complete range: overview rows, then commits, sequentially so a range
   *  load occupies at most one query admission slot. */
  async function readRange(range: Range, signal: AbortSignal) {
    const display_points = displayPoints;
    const overview = await query<Overview>("overview", { range, limit: 500, display_points }, signal);
    signal.throwIfAborted();
    const raw = await pages<Coverage>("overview", { range, display_points }, signal, overview);
    signal.throwIfAborted();
    const commits = await readCommits(range, signal);
    signal.throwIfAborted();
    return { overview, raw, commits };
  }
  async function loadRange(background = false) {
    if (!view.visible) return;
    cancelRangeTimer();
    const generation = ++rangeGeneration;
    rangeAbort.abort();
    rangeAbort = new AbortController();
    const signal = rangeAbort.signal, range = { ...view.visible };
    if (!background) leave();
    // Retain the old rows and images during a range change; Svelte repositions them.
    patch({ rangeLoading: true });
    try {
      const { overview, raw, commits } = await readRange(range, signal);
      if (generation !== rangeGeneration) return;
      publishRows(raw, overview, overview.revision, range, commits);
    } catch (error) {
      if (generation === rangeGeneration) { patch({ rangeLoading: false }); failure(error); }
    }
  }
  async function refresh() {
    if (disposed || !active || document.hidden || !view.visible || view.dayLoading || view.rangeLoading || refreshing || !revision) return;
    refreshing = true;
    const generation = rangeGeneration, signal = rangeAbort.signal;
    const args = { range: { ...view.visible }, display_points: displayPoints, since: revision, limit: 500 };
    try {
      let update = await query<OverviewUpdates>("overview_updates", args, signal);
      if (generation !== rangeGeneration) return;
      if (update.reset) { await loadRange(true); return; }
      const changes = [...update.changes.items];
      let bytes = JSON.stringify(update).length;
      while (update.changes.next_cursor) {
        update = await query<OverviewUpdates>("overview_updates", { ...args, cursor: update.changes.next_cursor }, signal);
        if (generation !== rangeGeneration) return;
        if (update.reset) { await loadRange(true); return; }
        changes.push(...update.changes.items);
        bytes += JSON.stringify(update).length;
        if (bytes > 16 * 1024 * 1024 || changes.length > 40000) throw new Error("Live update exceeds display limit. Zoom in.");
      }
      if (generation !== rangeGeneration) return;
      if (changes.length) {
        const rows = new Map(rawRows.map(row => [JSON.stringify(row), row]));
        for (const change of changes) {
          const key = JSON.stringify(change.row);
          if (change.remove) rows.delete(key); else rows.set(key, change.row);
        }
        const raw = [...rows.values()].sort((a, b) => a.kind.localeCompare(b.kind)
          || (a.range?.start_ms ?? 0) - (b.range?.start_ms ?? 0) || (a.segment ?? 0) - (b.segment ?? 0));
        publishRows(raw, { totals: update.totals ?? view.totals!, metrics: update.metrics ?? view.metrics! },
          update.revision, args.range);
        if (hoverActive && !hoverPending && hoverTimer === undefined) hoverTimer = setTimeout(flushHover, 100);
      } else revision = update.revision;
      // New commits do not change the overview; check them about once a minute.
      if (Date.now() - commitsRead >= 60_000) {
        const commits = await readCommits(args.range, signal);
        if (generation === rangeGeneration) patch(commits);
      }
    } catch (error) {
      if (generation === rangeGeneration && !signal.aborted) {
        if (error instanceof Error && error.message === "cursor_invalidated") await loadRange(true);
        else failure(error);
      }
    } finally { refreshing = false; }
  }
  function becameVisible() { if (!document.hidden) void refresh(); }
  /** Opens `date` around `center`, or on its recorded activity; `anchor` places
   *  the center within the visible range, which then stays within the day. */
  async function loadDay(date: string, center?: number, selected?: number, anchor?: number) {
    if (!date) return;
    const intent = ++interactionGeneration;
    revision = "";
    rawRows = [];
    cancelNavigation();
    cancelRangeTimer();
    const generation = ++rangeGeneration;
    rangeAbort.abort();
    rangeAbort = new AbortController();
    const signal = rangeAbort.signal;
    cancelSelection();
    leave();
    // Keep the last complete timeline while resolving a date and fetching its
    // replacement. A drag has already moved visible; explicit date navigation
    // commits the new scale and rows together, so labels never jump ahead.
    patch({ date, dayLoading: true, rangeLoading: true,
      evidenceLoading: false, highlight: null });
    try {
      let resolved = resolvedDays.get(date);
      if (!resolved) {
        resolved = await query<Overview["resolved"]>("resolve_range", { date }, signal);
        signal.throwIfAborted();
        resolvedDays.set(date, resolved);
        if (resolvedDays.size > 32) resolvedDays.delete(resolvedDays.keys().next().value!);
      }
      const day = resolved.range;
      let zoom = center === undefined ? "1" : view.zoom;
      let fitWidth = center === undefined ? undefined : view.fitWidth;
      let middle = center ?? (day.start_ms + day.end_ms) / 2;
      // A chosen date opens on its recorded activity, so one scan covers what is shown.
      if (center === undefined) {
        // Without bounds the day opens whole; they only frame it.
        const bounds = await query<{ activity?: Range | null }>("activity_bounds", { date }, signal)
          .catch(error => { signal.throwIfAborted(); void error; return null; });
        signal.throwIfAborted();
        if (bounds?.activity) {
          zoom = "fit";
          fitWidth = fittedWidth(day, bounds.activity);
          middle = (bounds.activity.start_ms + bounds.activity.end_ms) / 2;
        }
      }
      const width = zoom === "fit" ? fitWidth! : windowWidth(day, zoom);
      const start = Math.round(anchor === undefined ? middle - width / 2
        : Math.max(day.start_ms, Math.min(day.end_ms - width, middle - width * anchor)));
      const range = { start_ms: start, end_ms: start + width };
      // Resolve boundaries without scanning a full day whose overview would be
      // discarded whenever the dragged range straddles two activity days.
      const { overview, raw, commits } = await readRange(range, signal);
      if (generation !== rangeGeneration) return;
      loadedDate = date;
      // A chosen date shows only its own evidence; a drag across midnight keeps the selection in view.
      const elsewhere = center === undefined && selected === undefined && view.evidenceTime !== null
        && (view.evidenceTime < day.start_ms || view.evidenceTime >= day.end_ms);
      publishRows(raw, overview, overview.revision, range, { ...commits,
        day, timezone: resolved.timezone, visible: range, zoom, fitWidth, dayLoading: false,
        ...(elsewhere ? { frames: null, shots: [], focus: [], shotsExpired: false, evidenceTime: null, evidenceError: "", skip: null } : {}),
        ...(elsewhere || view.frames === null ? { selected: selected ?? range.start_ms } : {}),
      });
      if (selected !== undefined && view.date === date && intent === interactionGeneration) await select(selected);
    } catch (error) {
      if (generation === rangeGeneration) {
        patch({ dayLoading: false, rangeLoading: false, ...(loadedDate ? { date: loadedDate } : {}) });
        failure(error);
      }
    }
  }
  async function select(ms: number) {
    interactionGeneration++;
    cancelNavigation();
    cancelSelection();
    const generation = selectionGeneration, signal = selectionAbort.signal;
    patch({ selected: ms, evidenceLoading: true });
    try {
      const evidence = await query<Evidence>("selection", { selected_ms: ms }, signal, generation);
      if (generation !== selectionGeneration) return;
      const unchanged = view.frames !== null && !view.evidenceError && view.evidenceTime === ms && displayedLayout === view.layout
        && JSON.stringify(evidence.screenshots) === JSON.stringify(view.shots);
      const frames = unchanged ? view.frames! : await images.load(evidence.screenshots, view.layout, signal);
      if (generation !== selectionGeneration) return;
      displayedLayout = view.layout;
      patch({ shots: evidence.screenshots, shotsExpired: evidence.screenshots_expired, focus: evidence.focus, frames, evidenceTime: ms, evidenceError: "" });
    } catch (error) {
      if (generation === selectionGeneration) {
        patch({ evidenceError: "Could not load the selected moment." + (view.frames !== null ? " Previous evidence remains displayed." : "") });
        failure(error);
      }
    } finally {
      if (generation === selectionGeneration) patch({ evidenceLoading: false });
    }
  }
  async function flushHover() {
    hoverTimer = undefined;
    if (!hoverActive || hoverPending) return;
    hoverPending = true;
    const ms = hoverTime, generation = ++hoverGeneration;
    const signal = hoverAbort.signal;
    patch({ hoverEvidence: { ...view.hoverEvidence, loading: true, error: "" } });
    try {
      const evidence = await query<Evidence>("selection", { selected_ms: ms }, signal, generation, "hover");
      const frames = await hoverImages.load(evidence.screenshots, "grid", signal);
      if (generation === hoverGeneration && hoverActive)
        patch({ hoverEvidence: { time: ms, focus: evidence.focus, frames, expired: evidence.screenshots_expired, error: "", loading: false } });
    } catch (error) {
      if (!signal.aborted && generation === hoverGeneration) {
        hoverImages.clear();
        patch({ hoverEvidence: { time: ms, focus: [], frames: [], expired: false, error: "Preview unavailable.", loading: false } });
      }
    } finally {
      if (generation === hoverGeneration) {
        hoverPending = false;
        if (hoverActive && hoverTime !== ms && hoverTimer === undefined) hoverTimer = setTimeout(flushHover, 100);
      }
    }
  }
  function hover(ms: number) {
    hoverTime = ms;
    hoverActive = true;
    // Sample the latest pointer continuously, at most one in-flight image set.
    if (!hoverPending && hoverTimer === undefined && view.hoverEvidence.time !== ms)
      hoverTimer = setTimeout(flushHover, 100);
  }
  function leave() {
    cancelHover();
    hoverActive = false;
    hoverGeneration++;
    hoverAbort.abort();
    hoverAbort = new AbortController();
    hoverPending = false;
    if (view.hoverEvidence.loading) patch({ hoverEvidence: { ...view.hoverEvidence, loading: false } });
  }
  function zoom(value: string, anchor = 0.5, center = view.visible ? (view.visible.start_ms + view.visible.end_ms) / 2 : view.selected, scrolling = false) {
    if (!view.day || view.dayLoading) return;
    cancelNavigation();
    patch({ zoom: value });
    setVisible(center, anchor, true);
    if (scrolling) {
      // Move the scale immediately; fetch only once the wheel gesture settles.
      cancelRangeTimer();
      rangeGeneration++;
      rangeAbort.abort();
      patch({ rangeLoading: true });
      rangeTimer = setTimeout(() => void loadRange(), 220);
    } else void loadRange();
  }
  function setWidth(width: number) {
    if (width <= 0) return;
    const points = Math.max(64, Math.min(2048, Math.ceil(width / 128) * 64));
    if (points === displayPoints) return;
    displayPoints = points;
    if (view.visible && !view.dayLoading) {
      cancelRangeTimer();
      rangeGeneration++;
      rangeAbort.abort();
      patch({ rangeLoading: true });
      rangeTimer = setTimeout(() => void loadRange(), 220);
    }
  }
  function panTo(center: number, settled = false) {
    if (!view.visible || !view.day || view.dayLoading) return;
    cancelNavigation();
    leave();
    setVisible(center);
    cancelRangeTimer();
    rangeGeneration++;
    rangeAbort.abort();
    patch({ rangeLoading: true });
    // Keep current tracks while dragging; fetch the final range on release.
    if (settled) settleRange();
  }
  function pan(direction: number) {
    if (!view.visible || view.dayLoading) return;
    cancelNavigation();
    const { start_ms, end_ms } = view.visible;
    setVisible((start_ms + end_ms) / 2 + direction * (end_ms - start_ms) / 2);
    settleRange();
  }
  const activityDate = (ms: number) => activityDateIn(ms, view.timezone);
  function settleRange() {
    if (!view.visible) return;
    const center = (view.visible.start_ms + view.visible.end_ms) / 2;
    const date = activityDate(center);
    if (date !== view.date) void loadDay(date, center);
    else void loadRange();
  }
  async function navigateTo(ms: number) {
    if (activityDate(ms) !== view.date) { await loadDay(activityDate(ms), ms, ms); return; }
    if (!view.visible || ms < view.visible.start_ms || ms >= view.visible.end_ms) {
      setVisible(ms);
      void loadRange();
    }
    await select(ms);
  }
  /** Marks a range on the timeline, brings it into view and selects its start. */
  async function reveal(range: Range) {
    const date = activityDate(range.start_ms);
    if (date !== view.date) {
      // A dragged range can straddle days; loading the other day clears marks.
      await loadDay(date, (range.start_ms + range.end_ms) / 2, range.start_ms);
      if (view.date === date) patch({ highlight: range });
      return;
    }
    patch({ highlight: range });
    if (view.visible && (range.start_ms < view.visible.start_ms || range.end_ms > view.visible.end_ms)) {
      setVisible((range.start_ms + range.end_ms) / 2);
      void loadRange();
    }
    await select(range.start_ms);
  }
  /** Where now sits on the timeline: near its end, since nothing follows it yet. */
  const NOW_ANCHOR = 0.85;
  /** Selects the newest capture of the current activity day, since the present
   *  moment has no screenshot until the next one; without one, now itself. */
  async function jumpNow() {
    navigationAbort.abort(); navigationAbort = new AbortController();
    const signal = navigationAbort.signal;
    const now = Date.now();
    let ms = now;
    try {
      const latest = await query<number | null>("adjacent_screenshot", { selected_ms: now + 1, direction: -1,
        ...(view.layout.startsWith("monitor:") ? { monitor: view.layout.slice(8) } : {}) }, signal);
      if (latest !== null && activityDate(latest) === activityDate(now)) ms = latest;
    } catch (error) {
      if (signal.aborted) return;
      failure(error);
    }
    if (activityDate(ms) !== view.date) { await loadDay(activityDate(ms), now, ms, NOW_ANCHOR); return; }
    setVisible(now, NOW_ANCHOR, true);
    void loadRange();
    await select(ms);
  }
  /** Opens the current activity day as the viewer opens: framed on its activity,
   *  with its newest capture selected. */
  async function openToday() {
    navigationAbort.abort(); navigationAbort = new AbortController();
    const signal = navigationAbort.signal;
    const today = activityDate(Date.now());
    let latest: number | null = null;
    try {
      latest = await query<number | null>("adjacent_screenshot", { selected_ms: Date.now() + 1, direction: -1 }, signal);
    } catch (error) {
      if (signal.aborted) return;
      failure(error);
    }
    await loadDay(today, undefined, latest !== null && activityDate(latest) === today ? latest : undefined);
  }
  function stepZoom(direction: number) {
    const index = view.zoom === "fit" ? zoomLevels.findIndex(level =>
      (level.durationMs ?? 86_400_000 / Number(level.value)) <= view.fitWidth!) : zoomLevels.findIndex(level => level.value === view.zoom);
    zoom(zoomLevels[Math.max(0, Math.min(zoomLevels.length - 1, index + direction))].value);
  }
  async function fitActivity() {
    navigationAbort.abort(); navigationAbort = new AbortController();
    const signal = navigationAbort.signal;
    patch({ navigationBusy: true });
    try {
      const result = await query<{ activity: Range | null }>("activity_bounds", { date: view.date }, signal);
      signal.throwIfAborted();
      if (!result.activity) { patch({ message: "No recorded activity on this day." }); return; }
      const range = result.activity;
      patch({ zoom: "fit", fitWidth: fittedWidth(view.day!, range) });
      setVisible((range.start_ms + range.end_ms) / 2);
      await loadRange();
    } catch (error) { failure(error); }
    finally { if (!signal.aborted) patch({ navigationBusy: false }); }
  }
  /** Steps to the next screenshot that looks different, or with `exact` to the very next one. */
  async function stepScreenshot(direction: number, exact = false) {
    navigationAbort.abort(); navigationAbort = new AbortController();
    const signal = navigationAbort.signal;
    patch({ navigationBusy: true });
    try {
      const from = view.selected;
      const options = { selected_ms: from, direction, ...(view.layout.startsWith("monitor:") ? { monitor: view.layout.slice(8) } : {}) };
      const step = exact
        ? { unix_ms: await query<number | null>("adjacent_screenshot", options, signal), skipped: 0, skipped_ms: 0 }
        : await query<{ unix_ms: number | null; skipped: number; skipped_ms: number }>("screenshot_step", options, signal);
      signal.throwIfAborted();
      if (step.unix_ms === null) patch({ message: direction < 0 ? "No earlier screenshot." : "No later screenshot." });
      else {
        await navigateTo(step.unix_ms);
        // Selecting ends this navigation; the selection says whether it landed.
        if (view.selected === step.unix_ms)
          patch({ skip: step.skipped ? { at: step.unix_ms, count: step.skipped, ms: step.skipped_ms } : null });
      }
    } catch (error) { failure(error); }
    finally { if (!signal.aborted) patch({ navigationBusy: false }); }
  }
  function shiftDate(delta: number) { void loadDay(addDays(view.date, delta)); }
  async function layout(value: string) {
    patch({ layout: value });
    // Reselect if a query is pending so layout cannot cancel missing evidence.
    if (view.evidenceLoading) { await select(view.selected); return; }
    cancelSelection();
    const generation = selectionGeneration, signal = selectionAbort.signal;
    patch({ evidenceLoading: true });
    try {
      const frames = await images.load(view.shots, value, signal);
      if (generation === selectionGeneration) {
        displayedLayout = value;
        patch({ frames });
      }
    } catch (error) { if (generation === selectionGeneration) failure(error); }
    finally { if (generation === selectionGeneration) patch({ evidenceLoading: false }); }
  }
  /** Takes the session's timezone and starts live updates, once. */
  function attach(session: Session) {
    if (attached) return;
    attached = true;
    patch({ timezone: session.timezone, settingsPath: session.settingsPath });
    refreshTimer = setInterval(() => void refresh(), 5000);
    document.addEventListener("visibilitychange", becameVisible);
    window.addEventListener("focus", becameVisible);
  }
  /** Opens the day view on `date`, or on the newest capture's activity day. */
  async function start(session: Session, date?: string) {
    attach(session);
    try {
      if (date) { await loadDay(date); return; }
      // Find the newest available capture across activity days, including when
      // recording is paused. Explicit navigation during startup takes priority.
      const latest = await query<number | null>("adjacent_screenshot", {
        selected_ms: Date.now() + 1, direction: -1,
      }, lifetime.signal);
      if (disposed || view.date) return;
      if (latest === null) await loadDay(activityDate(Date.now()));
      else await loadDay(activityDate(latest), undefined, latest);
    } catch (error) { failure(error); }
  }
  /** Shows or hides the day view; hiding stops hover work and live polling. */
  function setActive(value: boolean) {
    active = value;
    if (!value) leave();
    else void refresh();
  }
  function dispose() {
    disposed = true;
    rangeGeneration++;
    rangeAbort.abort();
    cancelSelection();
    leave();
    lifetime.abort();
    navigationAbort.abort();
    clearInterval(refreshTimer);
    cancelRangeTimer();
    document.removeEventListener("visibilitychange", becameVisible);
    window.removeEventListener("focus", becameVisible);
    images.clear();
    hoverImages.clear();
  }
  return { state: readonly(store), start, dispose, loadDay, select, hover, leave, zoom, pan, panTo, setWidth, shiftDate, jumpNow, openToday, reveal, fitActivity, stepScreenshot, stepZoom, navigateTo, layout, attach, setActive };
}
export type TimelineApp = ReturnType<typeof createTimelineApp>;
