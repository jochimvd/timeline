<script lang="ts">
  import Icon from "./Icon.svelte";
  import { clock, duration } from "./format";
  import { plural } from "./metrics";
  import { initial, type Painter } from "./paint";
  import { gapLabel } from "./trends";
  import type { DayEvidence, FocusSession, Range } from "./types";
  /** The day as a chronological list of sessions, with breaks between them. */
  let { sessions, evidence, paint, timezone, selected, loading, error, onreveal }: {
    sessions: FocusSession[] | null; evidence: DayEvidence | null; paint: Painter; timezone: string;
    selected: number; loading: boolean; error: string;
    onreveal: (range: Range) => void;
  } = $props();
  /** Sessions shorter than this fold together when several follow each other. */
  const SHORT_MS = 2 * 60_000;
  /** Pauses between sessions at least this long are shown as breaks. */
  const BREAK_MS = 5 * 60_000;
  type Item =
    | { kind: "session"; key: string; session: FocusSession }
    | { kind: "short"; key: string; sessions: FocusSession[]; range: Range }
    | { kind: "break"; key: string; range: Range; label: string };
  let expanded = $state(new Set<string>());

  function breakLabel(range: Range) {
    let gap = 0, unknown = 0;
    const reasons = new Map<string, number>();
    for (const interval of evidence?.intervals ?? []) {
      const overlap = Math.min(interval.range.end_ms, range.end_ms) - Math.max(interval.range.start_ms, range.start_ms);
      if (overlap <= 0) continue;
      if (interval.kind === "gap") { gap += overlap; reasons.set(interval.reason ?? "", (reasons.get(interval.reason ?? "") ?? 0) + overlap); }
      else if (interval.kind === "unknown") unknown += overlap;
    }
    const length = range.end_ms - range.start_ms;
    if (gap >= length / 2) {
      const reason = [...reasons].sort((a, b) => b[1] - a[1])[0]?.[0] ?? "";
      const text = gapLabel(reason);
      return text[0].toUpperCase() + text.slice(1);
    }
    if (unknown >= length / 2) return "Not recorded";
    return "Idle or away";
  }
  const items = $derived.by(() => {
    const result: Item[] = [];
    const list = sessions ?? [];
    let previous: FocusSession | undefined;
    for (let index = 0; index < list.length; index++) {
      const session = list[index];
      if (previous && session.range.start_ms - previous.range.end_ms >= BREAK_MS) {
        const range = { start_ms: previous.range.end_ms, end_ms: session.range.start_ms };
        result.push({ kind: "break", key: "break:" + range.start_ms, range, label: breakLabel(range) });
      }
      const next = list[index + 1];
      const short = (s: FocusSession | undefined) => !!s && s.elapsed_ms < SHORT_MS;
      const last = result.at(-1);
      if (short(session) && last?.kind === "short" && session.range.start_ms - last.range.end_ms < BREAK_MS) {
        last.sessions.push(session);
        last.range = { ...last.range, end_ms: session.range.end_ms };
      } else if (short(session) && short(next) && next!.range.start_ms - session.range.end_ms < BREAK_MS) {
        result.push({ kind: "short", key: "short:" + session.range.start_ms, sessions: [session], range: { ...session.range } });
      } else result.push({ kind: "session", key: "session:" + session.range.start_ms, session });
      previous = session;
    }
    return result;
  });
  const contains = (range: Range) => range.start_ms <= selected && selected < range.end_ms;
  const activeKey = $derived(items.find(item => (item.kind === "session" ? contains(item.session.range) : item.kind === "short" && contains(item.range)))?.key);
  let list: HTMLOListElement | undefined = $state();
  let pointerInside = false;
  $effect(() => {
    const key = activeKey;
    if (!key || pointerInside || !list) return;
    // Scroll only the panel, never the page.
    const item = list.querySelector<HTMLElement>(`[data-key="${key}"]`);
    const scroller = list.closest<HTMLElement>(".panel-body");
    if (!item || !scroller || scroller.scrollHeight <= scroller.clientHeight) return;
    const top = item.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    if (top < 0 || top + item.offsetHeight > scroller.clientHeight)
      scroller.scrollTop += top - scroller.clientHeight / 2 + item.offsetHeight / 2;
  });
  function toggle(key: string) {
    const next = new Set(expanded);
    if (next.has(key)) next.delete(key); else next.add(key);
    expanded = next;
  }
  const first = $derived(sessions?.[0]);
  const last = $derived(sessions?.at(-1));
</script>

{#snippet sessionRow(session: FocusSession, key: string, nested = false)}
  {@const p = paint.session(session)}
  <li data-key={key} class:nested>
    <button class="session" class:active={contains(session.range)} onclick={() => onreveal(session.range)}
      aria-label={`${paint.sessionTitle(session)}, ${clock(session.range.start_ms, timezone)} to ${clock(session.range.end_ms, timezone)}, ${duration(session.elapsed_ms)}`}>
      <span class="time num">{clock(session.range.start_ms, timezone)}</span>
      <span class="rail" style:background={p.swatch.fill}></span>
      <span class="body">
        <span class="line">
          <strong class="ellipsis">{paint.sessionTitle(session)}</strong>
          <span class="length num">{duration(session.elapsed_ms)}</span>
        </span>
        {#each session.windows.slice(0, nested ? 1 : 2) as window (window.application + window.title)}
          <span class="window ellipsis" title={window.application + " · " + window.title}>
            <span class="mini" aria-hidden="true" style:background={paint.application(window.application).swatch.fill}>{initial(window.application)}</span>{window.title || window.application}
          </span>
        {/each}
        {#if session.interruptions && !nested}<span class="meta">{plural(session.interruptions, "brief glance")} away</span>{/if}
      </span>
    </button>
  </li>
{/snippet}

<div class="journal" aria-busy={loading}>
  {#if error}
    <p class="note error">{error}</p>
  {:else if sessions === null}
    <ol class="skeleton" aria-label="Loading sessions">{#each Array(6) as _, index (index)}<li></li>{/each}</ol>
  {:else if !sessions.length}
    <div class="note"><Icon name="list" size={22} /><p>No active focus recorded on this day.</p></div>
  {:else}
    <p class="span muted num">{clock(first!.range.start_ms, timezone)} – {clock(last!.range.end_ms, timezone)} · {plural(sessions.length, "session")}</p>
    <ol bind:this={list} id="sessions" onpointerenter={() => pointerInside = true} onpointerleave={() => pointerInside = false}>
      {#each items as item (item.key)}
        {#if item.kind === "session"}
          {@render sessionRow(item.session, item.key)}
        {:else if item.kind === "break"}
          <li class="break" data-key={item.key}>
            <span class="time num">{clock(item.range.start_ms, timezone)}</span>
            <span class="break-line"></span>
            <span class="break-label">{item.label} · {duration(item.range.end_ms - item.range.start_ms)}</span>
          </li>
        {:else}
          {@const total = item.sessions.reduce((sum, s) => sum + s.elapsed_ms, 0)}
          <li data-key={item.key}>
            <button class="session short" class:active={contains(item.range)} aria-expanded={expanded.has(item.key)} onclick={() => toggle(item.key)}>
              <span class="time num">{clock(item.range.start_ms, timezone)}</span>
              <span class="rail stack">
                {#each item.sessions.slice(0, 6) as s (s.range.start_ms)}<span style:background={paint.session(s).swatch.fill}></span>{/each}
              </span>
              <span class="body">
                <span class="line">
                  <span class="ellipsis">{item.sessions.length} short switches</span>
                  <span class="length num">{duration(total)}</span>
                </span>
                <span class="window ellipsis">{[...new Set(item.sessions.map(s => paint.session(s).label))].slice(0, 4).join(", ")}</span>
              </span>
              <span class="chev" class:open={expanded.has(item.key)}><Icon name="chevron-down" size={14} /></span>
            </button>
            {#if expanded.has(item.key)}
              <ol class="inner">{#each item.sessions as s (s.range.start_ms)}{@render sessionRow(s, "session:" + s.range.start_ms, true)}{/each}</ol>
            {/if}
          </li>
        {/if}
      {/each}
    </ol>
  {/if}
</div>

<style>
  .journal { display: flex; flex-direction: column; min-height: 0; }
  .span { font-size: 12px; padding: 0 4px 8px; }
  ol { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
  .session {
    display: grid; grid-template-columns: 44px 4px 1fr auto; gap: 10px; align-items: stretch; width: 100%;
    padding: 8px 8px 8px 4px; border: 0; border-radius: 8px; background: transparent; text-align: left; cursor: pointer;
    color: var(--text);
  }
  .session:hover { background: var(--surface-2); }
  .session.active { background: var(--accent-soft); }
  .time { font-size: 12px; color: var(--muted); padding-top: 1px; text-align: right; }
  .rail { border-radius: 2px; min-height: 18px; }
  .rail.stack { display: flex; flex-direction: column; gap: 2px; background: none; }
  .rail.stack span { flex: 1; border-radius: 2px; }
  .body { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
  .line { display: flex; justify-content: space-between; gap: 8px; min-width: 0; }
  .line strong { font-weight: 600; font-size: 13px; }
  .length { font-size: 12px; color: var(--text-2); flex: none; }
  .window { font-size: 12px; color: var(--muted); display: flex; align-items: center; gap: 6px; }
  .mini {
    display: inline-grid; place-items: center; width: 14px; height: 14px; border-radius: 4px; flex: none;
    font-size: 8.5px; font-weight: 700; color: var(--ink-on-fill);
  }
  .meta { font-size: 11.5px; color: var(--faint); }
  .short .ellipsis { color: var(--text-2); font-size: 12.5px; }
  .chev { color: var(--muted); align-self: center; transition: transform 0.15s; }
  .chev.open { transform: rotate(180deg); }
  .session:not(.short) { grid-template-columns: 44px 4px 1fr; }
  .inner { padding-left: 12px; margin: 2px 0 4px; border-left: 1px dashed var(--border-strong); margin-left: 50px; }
  .nested .session { grid-template-columns: 38px 4px 1fr; padding-block: 5px; }
  .break { display: grid; grid-template-columns: 44px 1fr; gap: 10px; align-items: center; padding: 6px 8px 6px 4px; }
  .break .time { color: var(--faint); }
  .break-label { font-size: 12px; color: var(--muted); display: flex; align-items: center; gap: 8px; }
  .break-label::before { content: ""; flex: 0 0 18px; border-top: 1px dashed var(--border-strong); }
  .break-line { display: none; }
  .note { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 32px 12px; color: var(--muted); text-align: center; }
  .note.error { color: var(--bad); }
  .skeleton li { height: 46px; border-radius: 8px; background: var(--surface-2); animation: pulse 1.4s ease-in-out infinite; }
  @keyframes pulse { 50% { opacity: 0.5; } }
</style>
