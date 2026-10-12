<script lang="ts">
  import Icon from "./Icon.svelte";
  import { tick, type Snippet } from "svelte";
  import { addDays, addMonths, dateLabel, dateSpan, daysInMonth, monthStart, weekNumber, weekStart, weekday } from "./dates";
  let { kind, value, today, max, id, title, onselect, children, sizes = [] }: {
    /** What is picked: an activity day, an activity week (by its Monday) or a calendar month (by its first day). */
    kind: "day" | "week" | "month";
    /** The picked day, or the first day of the picked week or month. */
    value: string; today: string;
    /** The last day that can be picked; a week or month starting later cannot be. */
    max: string;
    id: string; title: string;
    onselect: (date: string) => void;
    /** The heading text on the trigger. */
    children: Snippet;
    /** Every heading the trigger can show, or the widest ones: it keeps their
     *  width, so the controls beside it stay put from one date to the next. */
    sizes?: string[];
  } = $props();

  let root: HTMLElement;
  let open = $state(false);
  /** Days and weeks are picked on a month's calendar; months on a year's, which
   *  also jumps the calendar to another month. */
  let zoom = $state<"days" | "months">("days");
  /** The first day of the month on the calendar. */
  let month = $state("");
  /** The keyboard position: a day, a week's Monday or a month's first day. */
  let cursor = $state("");

  const unit = (date: string) => kind === "week" ? weekStart(date) : kind === "month" ? monthStart(date) : date;
  /** Six rows, so the calendar keeps its height from month to month. */
  const days = $derived(month ? Array.from({ length: 42 }, (_, index) => addDays(weekStart(month), index)) : []);
  const weeks = $derived(Array.from({ length: 6 }, (_, index) => days.slice(index * 7, index * 7 + 7)));
  const months = $derived(month ? Array.from({ length: 12 }, (_, index) => `${month.slice(0, 4)}-${String(index + 1).padStart(2, "0")}-01`) : []);
  const late = (first: string) => first > max;
  const step = $derived(zoom === "months" ? 12 : 1);
  const heading = $derived(zoom === "months" ? month.slice(0, 4) : dateLabel(month, { month: "long", year: "numeric" }));
  const noun = $derived(zoom === "months" ? "year" : "month");

  async function show() {
    const first = unit(value);
    cursor = first;
    zoom = kind === "month" ? "months" : "days";
    month = monthStart(kind === "week" ? addDays(first, 3) : first);
    open = true;
    await focusCursor();
  }
  function close(refocus = true) {
    open = false;
    if (refocus) root.querySelector<HTMLButtonElement>(".trigger")?.focus();
  }
  async function focusCursor() {
    await tick();
    root.querySelector<HTMLButtonElement>(`.panel [data-date="${cursor}"]`)?.focus();
  }
  function pick(date: string) {
    if (late(date)) return;
    close();
    if (date !== unit(value)) onselect(date);
  }
  /** On the year's calendar, a day or week picker opens the chosen month. */
  async function chooseMonth(first: string) {
    if (kind === "month") return pick(first);
    month = first;
    zoom = "days";
    cursor = kind === "week" ? weekStart(addDays(first, (10 - weekday(first)) % 7)) : first;
    await focusCursor();
  }
  function page(delta: number) {
    month = addMonths(month, delta * step);
  }
  /** Places the cursor, bringing its month onto the calendar. */
  async function moveTo(date: string) {
    if (late(date)) return;
    cursor = date;
    const shown = zoom === "months" ? date.slice(0, 4) === month.slice(0, 4) : days.includes(date) && (kind === "week" || date.slice(0, 7) === month.slice(0, 7));
    if (!shown) month = zoom === "months" ? date.slice(0, 5) + "01-01" : monthStart(kind === "week" ? addDays(date, 3) : date);
    await focusCursor();
  }
  function key(event: KeyboardEvent) {
    if (!open) return;
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); return; }
    if (!(event.target as HTMLElement).closest(".grid")) return;
    const moves: Record<string, number> = zoom === "months" ? { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -3, ArrowDown: 3 }
      : kind === "week" ? { ArrowLeft: -7, ArrowRight: 7, ArrowUp: -7, ArrowDown: 7 } : { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7 };
    let target: string | undefined;
    if (event.key in moves) target = zoom === "months" ? addMonths(cursor, moves[event.key]) : addDays(cursor, moves[event.key]);
    else if (event.key === "PageUp" || event.key === "PageDown") {
      // The same day of the previous or next month (or year), or its last day.
      const other = addMonths(cursor, (event.key === "PageUp" ? -1 : 1) * step);
      target = zoom === "months" ? other : unit(other.slice(0, 8) + String(Math.min(Number(cursor.slice(8)), daysInMonth(other))).padStart(2, "0"));
    } else if (event.key === "Home") target = unit(today);
    if (!target) return;
    event.preventDefault();
    event.stopPropagation();
    void moveTo(target);
  }
  /** Clicking or focusing elsewhere closes the picker; a re-rendered calendar does not. */
  function outside(event: Event) {
    if (open && !root.contains(event.target as Node)) close(false);
  }
</script>

<svelte:document onpointerdown={outside} onfocusin={outside} />

<div class="picker" bind:this={root} onkeydown={key} role="presentation">
  <h1>
    <button type="button" class="trigger" {id} {value} {title} aria-haspopup="dialog" aria-expanded={open} onclick={() => open ? close() : void show()}>
      <span class="heading">
        <span class="shown">{@render children()}<Icon name="chevron-down" size={14} /></span>
        {#each sizes as size (size)}<span class="size" aria-hidden="true">{size}<span class="chevron"></span></span>{/each}
      </span>
    </button>
  </h1>
  {#if open}
    <div class="panel" role="dialog" aria-label={title} tabindex="-1">
      <div class="bar">
        <button type="button" class="btn small icon ghost" aria-label={`Previous ${noun}`} onclick={() => page(-1)}><Icon name="chevron-left" size={14} /></button>
        {#if zoom === "days"}
          <button type="button" class="btn small ghost caption" aria-label={`${heading}, choose another month`} onclick={() => { zoom = "months"; cursor = month; void focusCursor(); }}>
            {heading}<Icon name="chevron-down" size={12} /></button>
        {:else}
          <span class="caption">{heading}</span>
        {/if}
        <button type="button" class="btn small icon ghost" aria-label={`Next ${noun}`} disabled={late(addMonths(zoom === "months" ? month.slice(0, 5) + "01-01" : month, step))}
          onclick={() => page(1)}><Icon name="chevron-right" size={14} /></button>
      </div>

      {#if zoom === "months"}
        <div class="grid months">
          {#each months as first}
            <button type="button" data-date={first} tabindex={first === cursor ? 0 : -1} disabled={late(first)}
              class:selected={kind === "month" && first === unit(value)} class:current={first === monthStart(today)}
              aria-label={dateLabel(first, { month: "long", year: "numeric" })} aria-current={first === monthStart(today) ? "date" : undefined}
              onclick={() => void chooseMonth(first)}>{dateLabel(first, { month: "short" })}</button>
          {/each}
        </div>
      {:else}
        <div class="weekdays" class:numbered={kind === "week"} aria-hidden="true">
          {#if kind === "week"}<span>Wk</span>{/if}
          {#each ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] as name}<span>{name}</span>{/each}
        </div>
        {#if kind === "week"}
          <div class="grid weeks">
            {#each weeks as week}
              <button type="button" class="week" data-date={week[0]} tabindex={week[0] === cursor ? 0 : -1} disabled={late(week[0])}
                class:selected={week[0] === unit(value)} class:current={week[0] === weekStart(today)}
                aria-label={`Week ${weekNumber(week[0])}, ${dateSpan(week[0], week[6])}`} aria-current={week[0] === weekStart(today) ? "date" : undefined}
                onclick={() => pick(week[0])}>
                <span class="number">{weekNumber(week[0])}</span>
                {#each week as date}<span class="day" class:outside={date.slice(0, 7) !== month.slice(0, 7)} class:today={date === today}>{Number(date.slice(8))}</span>{/each}
              </button>
            {/each}
          </div>
        {:else}
          <div class="grid days">
            {#each days as date}
              <button type="button" class="day" data-date={date} tabindex={date === cursor ? 0 : -1} disabled={late(date)}
                class:outside={date.slice(0, 7) !== month.slice(0, 7)} class:selected={date === value} class:today={date === today}
                aria-label={dateLabel(date, { weekday: "long", day: "numeric", month: "long", year: "numeric" })} aria-current={date === today ? "date" : undefined}
                onclick={() => pick(date)}>{Number(date.slice(8))}</button>
            {/each}
          </div>
        {/if}
      {/if}
    </div>
  {/if}
</div>

<style>
  .picker { position: relative; }
  .trigger {
    display: flex; align-items: center; gap: 6px; padding: 2px 8px; border: 0; border-radius: 8px; background: none;
    font: inherit; color: var(--text); cursor: pointer;
  }
  /* The headings share one cell, which takes the widest; the shown one is centred. */
  .heading { display: inline-grid; justify-items: center; font-variant-numeric: tabular-nums; }
  .heading > * { grid-area: 1 / 1; display: flex; align-items: center; gap: 6px; }
  .shown > :global(svg) { flex: none; color: var(--muted); }
  .size { visibility: hidden; }
  .chevron { width: 14px; }
  .trigger:hover, .trigger[aria-expanded="true"] { background: var(--surface-2); }
  .trigger:focus-visible { outline: 2px solid var(--accent); }
  h1 { font-size: 24px; font-weight: 650; letter-spacing: -0.02em; white-space: nowrap; }

  .panel {
    position: absolute; top: calc(100% + 6px); left: 0; z-index: 30; width: max-content; padding: 10px;
    border-radius: var(--radius); background: var(--surface-2); box-shadow: var(--popover-shadow);
    animation: appear 0.12s ease-out; outline: none;
  }
  @keyframes appear { from { opacity: 0; transform: translateY(-4px); } }
  .bar { display: flex; align-items: center; justify-content: space-between; gap: 4px; margin-bottom: 8px; }
  .caption { font-size: 13px; font-weight: 600; color: var(--text); padding: 0 8px; }
  .bar button.caption { gap: 4px; }

  .weekdays, .week, .days { display: grid; grid-template-columns: repeat(7, 34px); }
  .weekdays.numbered, .week { grid-template-columns: 30px repeat(7, 34px); }
  .weekdays { font-size: 11px; font-weight: 500; color: var(--faint); text-align: center; padding-bottom: 4px; }
  .grid button {
    border: 0; background: none; color: var(--text-2); font: inherit; font-size: 12.5px; font-variant-numeric: tabular-nums;
    cursor: pointer; border-radius: 8px; transition: background 0.1s, color 0.1s;
  }
  .grid button:disabled { color: var(--faint); opacity: 0.5; cursor: default; }
  .grid button:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  .grid button:hover:not(:disabled) { background: var(--surface-3); color: var(--text); }
  .grid .selected, .grid .selected:hover:not(:disabled) { background: var(--accent); color: #fff; }

  .days { gap: 2px 0; }
  .days .day { height: 32px; }
  .day.outside { color: var(--faint); }
  .day.today:not(.selected) { color: var(--accent-strong); font-weight: 650; }

  .weeks { display: flex; flex-direction: column; gap: 2px; }
  .week { height: 32px; padding: 0; align-items: center; text-align: center; }
  .week .number { font-size: 11px; color: var(--faint); }
  .week.current:not(.selected) .number { color: var(--accent-strong); font-weight: 650; }
  .week.selected .number, .week.selected .day { color: #fff; }

  .months { display: grid; grid-template-columns: repeat(3, 80px); gap: 4px; }
  .months button { height: 40px; }
  .months .current:not(.selected) { color: var(--accent-strong); font-weight: 650; box-shadow: inset 0 0 0 1px var(--accent-soft); }

  @media (max-width: 700px) { h1 { font-size: 19px; } }
</style>
