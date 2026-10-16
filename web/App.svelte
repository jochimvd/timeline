<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import DayView from "./DayView.svelte";
  import Disconnected from "./Disconnected.svelte";
  import Icon from "./Icon.svelte";
  import PeriodView from "./PeriodView.svelte";
  import SettingsDrawer from "./SettingsDrawer.svelte";
  import Toasts from "./Toasts.svelte";
  import { query, request } from "./api";
  import { connection, lostLabel, SignedOut } from "./connection";
  import { categoryNodes, grouping, isNested, type CategoryNode } from "./categories";
  import { activityDate, dateLabel } from "./dates";
  import { createTimelineApp } from "./model";
  import { createPeriodModel } from "./period";
  import { drawer } from "./drawer";
  import { colorBy, type ColorBy } from "./preferences";
  import { createRecording } from "./recording";
  import { containsDate, parseRoute, period, periodTitle, routeSearch, sameRoute, shiftPeriod, type Route } from "./routes";
  import { startSession, type Session } from "./session";
  import { dismiss, notify } from "./toasts";
  import type { CategoryList, Range } from "./types";
  const app = createTimelineApp();
  const day = app.state;
  const periods = createPeriodModel();
  const periodState = periods.state;
  const recording = createRecording();
  const status = recording.state;
  let session = $state<Session | null>(null);
  let route = $state<Route>(parseRoute(location.search));
  let categoryList = $state<CategoryList | null>(null);
  let dayAttached = false;
  const selected = $derived(route.view === "day" ? null : period(route.view, route.date));
  /** The view on screen. Switching between a day and a week or month keeps the
   *  previous view on screen until the next has loaded, as each view keeps its
   *  previous day or period, so the page changes once instead of in steps. */
  let shown = $state<"day" | "period">(untrack(() => route.view) === "day" ? "day" : "period");
  const periodReady = $derived(!!selected && !$periodState.loading && $periodState.period?.kind === selected.kind && $periodState.period.first === selected.first);
  $effect(() => {
    if (route.view === "day") { if (!$day.dayLoading) shown = "day"; }
    else if (periodReady) shown = "period";
  });
  /** Set when drilling into a day: the timeline takes focus once it shows. */
  let drilled = false;
  $effect(() => {
    if (shown !== "day" || !drilled) return;
    drilled = false;
    void tick().then(() => {
      window.scrollTo({ top: 0 });
      document.getElementById("timeline")?.focus({ preventScroll: true });
    });
  });
  const pending = $derived(!!session && !$connection && (selected ? shown === "day" || $periodState.loading : shown === "period"));
  const timezone = $derived(session?.timezone ?? $day.timezone);
  let now = $state(Date.now());
  const today = $derived(activityDate(now, timezone));
  const nodes = $derived<CategoryNode[]>(categoryList ? categoryNodes(categoryList.categories.map(c => ({ ...c, category: c.name }))) : []);
  const choice = $derived(grouping($colorBy, nodes));

  /** Shows `next`, recording it in browser history unless it came from there. */
  function show(next: Route, mode: "push" | "replace" | "none") {
    if (mode !== "none" && !sameRoute(next, parseRoute(location.search)))
      history[mode === "push" ? "pushState" : "replaceState"](null, "", routeSearch(next));
    route = next;
    drilled = false;
    apply();
  }
  function apply() {
    if (!session) return;
    // An error belongs to the view it came from.
    dismiss(route.view === "day" ? "period" : "day");
    if (route.view === "day") {
      periods.setActive(false);
      app.setActive(true);
      if (!dayAttached) { dayAttached = true; void app.start(session, route.date); }
      else if (route.date && route.date !== $day.date) void app.loadDay(route.date);
    } else {
      app.setActive(false);
      periods.setActive(true);
      const next = period(route.view, route.date);
      if ($periodState.period?.kind !== next.kind || $periodState.period.first !== next.first || !$periodState.current)
        void periods.load(next);
    }
  }
  /** Drills into a day, optionally marking a range such as a session. */
  function openDay(date: string, range?: Range) {
    if (!session) return;
    history.pushState(null, "", routeSearch({ view: "day", date }));
    route = { view: "day", date };
    dismiss("period");
    periods.setActive(false);
    app.setActive(true);
    if (!dayAttached) { dayAttached = true; app.attach(session); }
    void (range ? app.reveal(range) : app.loadDay(date));
    drilled = true;
  }
  function changePeriod(date: string | null, delta = 0) {
    if (!selected) return;
    const target = date === null ? period(selected.kind, today) : shiftPeriod(period(selected.kind, date), delta);
    show({ view: target.kind, date: target.first }, "replace");
  }
  /** Where the view switch leads: the same moment in another view. */
  const anchor = $derived(route.view === "day" ? ($day.date || today)
    : selected && containsDate(selected, today) ? today : selected?.first ?? today);
  const views = $derived([
    { label: "Day", route: { view: "day", date: anchor } as Route },
    { label: "Week", route: { view: "week", date: period("week", anchor).first } as Route },
    { label: "Month", route: { view: "month", date: period("month", anchor).first } as Route },
  ]);
  function follow(event: MouseEvent, target: Route) {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    if (target.view !== route.view) { show(target, "push"); window.scrollTo({ top: 0 }); }
  }
  const colorChoices = $derived<{ value: ColorBy; label: string }[]>([
    { value: "category", label: "Category" },
    ...(isNested(nodes) ? [{ value: "subcategory" as const, label: "Subcategory" }] : []),
    { value: "application", label: "App" },
  ]);
  // The day view's date follows its navigation; record it so a reload returns there.
  $effect(() => {
    const date = $day.date;
    if (route.view === "day" && date && route.date !== date) untrack(() => show({ view: "day", date }, "replace"));
  });
  $effect(() => {
    document.title = $connection ? `${lostLabel[$connection]} · Timeline` : (selected ? periodTitle(selected)
      : $day.shown ? dateLabel($day.shown, { weekday: "short", day: "numeric", month: "short" }) : "Timeline")
      + (selected || $day.shown ? " · Timeline" : "");
  });
  onMount(() => {
    const lifetime = new AbortController();
    const popstate = () => show(parseRoute(location.search), "none");
    window.addEventListener("popstate", popstate);
    const clock = setInterval(() => now = Date.now(), 60_000);
    // Canonical URL: a period by its first day, malformed parameters dropped.
    if (routeSearch(route) !== (location.search || "/")) history.replaceState(null, "", routeSearch(route) + location.hash);
    startSession(lifetime.signal).then(started => {
      session = started;
      periods.start(started);
      apply();
      void recording.start();
      query<CategoryList>("categories", {}, lifetime.signal).then(list => categoryList = list).catch(() => {});
    }).catch(failure => {
      if (!lifetime.signal.aborted && !(failure instanceof SignedOut)) notify(failure instanceof Error ? failure.message : String(failure), { key: "session", tone: "error" });
    });
    return () => {
      lifetime.abort(); clearInterval(clock); window.removeEventListener("popstate", popstate);
      app.dispose(); periods.dispose(); recording.dispose();
    };
  });
  // Signed out, the page asks the server again every few seconds and whenever
  // it is shown, so a session started in another tab reconnects this one.
  $effect(() => {
    if (!$connection) return;
    let asking = false;
    const ask = () => {
      if (asking || document.hidden) return;
      asking = true;
      void request("/api/config").catch(() => {}).finally(() => asking = false);
    };
    const timer = setInterval(ask, 3000);
    window.addEventListener("focus", ask);
    document.addEventListener("visibilitychange", ask);
    return () => { clearInterval(timer); window.removeEventListener("focus", ask); document.removeEventListener("visibilitychange", ask); };
  });
  // Reconnected, every view starts afresh from the server.
  let wasLost = false;
  $effect(() => {
    const lost = !!$connection;
    if (wasLost && !lost) location.reload();
    wasLost = lost;
  });
  // A failed pause or resume says why until dismissed.
  const statusError = $derived($status.error);
  $effect(() => { if (statusError) untrack(() => notify(statusError, { key: "recording", tone: "error" })); });
</script>

<div class="shell" class:fill={shown === "day" || !!$connection}>
  <header class="topbar">
    <a class="brand" href="/" onclick={event => follow(event, { view: "day", date: today })} aria-label="Timeline, today">
      <svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true">
        <rect x="2" y="5" width="7" height="4" rx="2" fill="var(--series-1)" />
        <rect x="10.5" y="10" width="11.5" height="4" rx="2" fill="var(--series-3)" />
        <rect x="5" y="15" width="9" height="4" rx="2" fill="var(--series-7)" />
      </svg>
      <span>Timeline</span>
    </a>
    <nav class="segmented views" aria-label="View">
      {#each views as entry (entry.label)}
        <a href={routeSearch(entry.route)} aria-current={entry.route.view === route.view ? "page" : undefined}
          onclick={event => follow(event, entry.route)}>{entry.label}</a>
      {/each}
    </nav>
    <div class="spacer"></div>
    {#if nodes.length}
      <div class="color-by" role="group" aria-label="Color by">
        <span class="label">Color by</span>
        <div class="segmented">
          {#each colorChoices as option (option.value)}
            <button aria-pressed={choice === option.value} onclick={() => colorBy.set(option.value)}>{option.label}</button>
          {/each}
        </div>
      </div>
    {/if}
    <div class="recording" class:paused={$status.paused && !$connection} class:off={$status.busy && !$status.paused} class:lost={!!$connection}>
      <span class="pulse" aria-hidden="true"></span>
      <span id="recording" class="recording-label">{$connection ? lostLabel[$connection] : $status.label}</span>
      <button id="pause" class="btn small ghost" disabled={$status.busy || !!$connection} onclick={() => void recording.toggle()}
        aria-label={$status.paused ? "Resume recording" : "Pause recording"} title={$status.paused ? "Resume recording" : "Pause recording"}>
        <Icon name={$status.paused ? "play" : "pause"} size={14} />{$status.paused ? "Resume" : "Pause"}
      </button>
    </div>
    <button class="btn icon ghost" aria-label="Settings and details" title="Settings and details" onclick={() => drawer.set("categories")}>
      <Icon name="settings" />
    </button>
    {#if pending}<div class="progress" role="progressbar" aria-label="Loading"></div>{/if}
  </header>

  <main>
    {#if $connection}
      <Disconnected reason={$connection} />
    {:else if shown === "day"}
      <DayView {app} {nodes} {choice} {today} />
    {:else if selected}
      <PeriodView model={periods} period={selected} {today} {choice} onperiod={changePeriod} onopen={openDay} />
    {/if}
  </main>
</div>

<Toasts />
<SettingsDrawer {categoryList} settingsPath={session?.settingsPath ?? null} {timezone} />

<style>
  .shell { min-height: 100vh; display: flex; flex-direction: column; }
  /* The day view fits the viewport exactly; week and month scroll as pages. */
  .shell.fill { height: 100dvh; min-height: 640px; }
  .shell.fill main { display: flex; flex-direction: column; min-height: 0; }
  @media (max-width: 1180px) { .shell.fill { height: auto; } }
  .topbar {
    position: sticky; top: 0; z-index: 20; display: flex; align-items: center; gap: 16px;
    height: 56px; padding: 0 20px; background: color-mix(in oklab, var(--bg), transparent 15%);
    backdrop-filter: blur(12px); border-bottom: 1px solid var(--border);
  }
  /* Sits on the top bar's border, so showing it moves nothing. */
  .progress {
    position: absolute; left: 0; right: 0; bottom: -1px; height: 2px; overflow: hidden; pointer-events: none;
  }
  .progress::after {
    content: ""; position: absolute; top: 0; bottom: 0; left: 0; width: 30%; background: var(--accent);
    animation: progress 1.1s ease-in-out infinite;
  }
  @keyframes progress { from { transform: translateX(-100%); } to { transform: translateX(333%); } }
  .brand { display: flex; align-items: center; gap: 9px; color: var(--text); text-decoration: none; font-weight: 650; font-size: 15px; letter-spacing: -0.01em; }
  .spacer { flex: 1; }
  .color-by { display: flex; align-items: center; gap: 8px; }
  .color-by .label { font-size: 12px; color: var(--muted); }
  .recording {
    display: flex; align-items: center; gap: 8px; height: 32px; padding: 0 4px 0 12px;
    border-radius: 999px; border: 1px solid var(--border); background: var(--surface);
  }
  .recording-label { font-size: 12.5px; color: var(--text-2); white-space: nowrap; }
  .pulse { position: relative; width: 8px; height: 8px; border-radius: 50%; background: var(--good); }
  .pulse::after {
    content: ""; position: absolute; inset: -4px; border-radius: 50%; border: 2px solid var(--good);
    opacity: 0; animation: ping 2.4s ease-out infinite;
  }
  .paused .pulse { background: var(--warn); }
  .paused .pulse::after, .off .pulse::after { display: none; }
  .off .pulse { background: var(--faint); }
  .lost .pulse { background: var(--bad); }
  .lost .pulse::after { display: none; }
  @keyframes ping { 0% { transform: scale(0.6); opacity: 0.7; } 80%, 100% { transform: scale(1.4); opacity: 0; } }
  main { flex: 1; padding: 20px; width: 100%; max-width: 1760px; margin: 0 auto; }
  @media (max-width: 900px) {
    .topbar { gap: 10px; padding: 0 12px; flex-wrap: wrap; height: auto; padding-block: 8px; }
    .color-by .label, .recording-label { display: none; }
    .brand span { display: none; }
    main { padding: 12px; }
  }
</style>
