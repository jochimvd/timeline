<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import DayView from "./DayView.svelte";
  import PeriodView from "./PeriodView.svelte";
  import RepositoryDetails from "./RepositoryDetails.svelte";
  import StorageDetails from "./StorageDetails.svelte";
  import { activityDate, dateLabel } from "./dates";
  import { createTimelineApp } from "./model";
  import { createPeriodModel } from "./period";
  import { createRecording } from "./recording";
  import { containsDate, parseRoute, period, periodTitle, routeSearch, sameRoute, shiftPeriod, type Route } from "./routes";
  import { startSession, type Session } from "./session";
  import type { Range } from "./types";
  const app = createTimelineApp();
  const day = app.state;
  const periods = createPeriodModel();
  const periodState = periods.state;
  const recording = createRecording();
  const status = recording.state;
  let session = $state<Session | null>(null);
  let error = $state("");
  let route = $state<Route>(parseRoute(location.search));
  let dayAttached = false;
  const selected = $derived(route.view === "day" ? null : period(route.view, route.date));
  const timezone = $derived(session?.timezone ?? $day.timezone);
  let now = $state(Date.now());
  const today = $derived(activityDate(now, timezone));

  /** Shows `next`, recording it in browser history unless it came from there. */
  function show(next: Route, mode: "push" | "replace" | "none") {
    if (mode !== "none" && !sameRoute(next, parseRoute(location.search)))
      history[mode === "push" ? "pushState" : "replaceState"](null, "", routeSearch(next));
    route = next;
    apply();
  }
  function apply() {
    if (!session) return;
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
  /** Drills into a day, optionally marking a range such as an hour or stretch. */
  async function openDay(date: string, range?: Range) {
    if (!session) return;
    history.pushState(null, "", routeSearch({ view: "day", date }));
    route = { view: "day", date };
    periods.setActive(false);
    app.setActive(true);
    if (!dayAttached) { dayAttached = true; app.attach(session); }
    void (range ? app.reveal(range) : app.loadDay(date));
    await tick();
    window.scrollTo({ top: 0 });
    document.getElementById("timeline")?.focus({ preventScroll: true });
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
  // The day view's date follows its navigation; record it so a reload returns there.
  $effect(() => {
    const date = $day.date;
    if (route.view === "day" && date && route.date !== date) untrack(() => show({ view: "day", date }, "replace"));
  });
  $effect(() => {
    document.title = (selected ? periodTitle(selected)
      : $day.date ? dateLabel($day.date, { weekday: "short", day: "numeric", month: "short", year: "numeric" }) : "Desktop timeline")
      + (selected || $day.date ? " · Desktop timeline" : "");
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
    }).catch(failure => {
      if (!lifetime.signal.aborted) error = failure instanceof Error ? failure.message : String(failure);
    });
    return () => {
      lifetime.abort(); clearInterval(clock); window.removeEventListener("popstate", popstate);
      app.dispose(); periods.dispose(); recording.dispose();
    };
  });
</script>

<main>
  <header class="app-header">
    <div><span class="eyebrow">DESKTOP ACTIVITY</span><h1>Your activity, recalled.</h1></div>
    <nav class="view-switch" aria-label="View">
      {#each views as entry (entry.label)}
        <a href={routeSearch(entry.route)} aria-current={entry.route.view === route.view ? "page" : undefined}
          onclick={event => follow(event, entry.route)}>{entry.label}</a>
      {/each}
    </nav>
    <section class="status" aria-label="Recording">
      <span id="recording">{$status.label}</span>
      <button id="pause" disabled={$status.busy} onclick={() => void recording.toggle()}>{$status.paused ? "Resume recording" : "Pause recording"}</button>
    </section>
  </header>
  <p id="message" role="status">{error || $status.error || (route.view === "day" ? $day.message : $periodState.message)}</p>
  {#if route.view === "day"}
    <DayView {app} />
  {:else if selected}
    <PeriodView model={periods} period={selected} {today} onperiod={changePeriod} onopen={(date, range) => void openDay(date, range)} />
  {/if}
  <RepositoryDetails settingsPath={session?.settingsPath ?? null} {timezone} />
  <StorageDetails />
  <footer>Activity days begin at 04:00; weeks begin on Monday. Closing this page leaves recording running.</footer>
</main>
