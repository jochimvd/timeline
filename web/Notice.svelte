<script lang="ts" module>
  /** A caveat about the data on screen: `label` on the badge, `text` in full. */
  export type Note = { label: string; text: string; warn?: boolean; action?: { label: string; run: () => void } };
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  let { id, notes }: { id: string; notes: Note[] } = $props();
  let root = $state<HTMLElement>();
  let open = $state(false);
  const warn = $derived(notes.some(note => note.warn));
  $effect(() => { if (!notes.length) open = false; });
  function close(refocus = true) {
    open = false;
    if (refocus) root?.querySelector<HTMLButtonElement>(".badge")?.focus();
  }
  function outside(event: Event) {
    if (open && root && !root.contains(event.target as Node)) close(false);
  }
</script>

<svelte:document onpointerdown={outside} onfocusin={outside} />

<!-- Sits at the end of a view's subtitle line, so a note never moves the page. -->
{#if notes.length}
  <span class="notice" bind:this={root} role="presentation"
    onkeydown={event => { if (event.key === "Escape" && open) { event.preventDefault(); event.stopPropagation(); close(); } }}>
    <button type="button" class="badge" class:warn aria-expanded={open} aria-controls={id} onclick={() => open = !open}>
      <Icon name={warn ? "alert" : "info"} size={13} />{notes.length === 1 ? notes[0].label : `${notes.length} notes`}
    </button>
    <span class="popover" {id} role="dialog" aria-label="About this data" hidden={!open}>
      {#each notes as note (note.label)}
        <span class="note" class:warn={note.warn}>
          <span>{note.text}</span>
          {#if note.action}
            {@const action = note.action}
            <button type="button" class="link" onclick={() => { close(false); action.run(); }}>{action.label}</button>
          {/if}
        </span>
      {/each}
    </span>
  </span>
{/if}

<style>
  .notice { position: relative; display: inline-flex; vertical-align: middle; }
  .badge {
    display: inline-flex; align-items: center; gap: 5px; height: 20px; padding: 0 7px; border: 0; border-radius: 999px;
    background: var(--surface-2); color: var(--text-2); font: inherit; font-size: 11.5px; font-weight: 500; white-space: nowrap; cursor: pointer;
  }
  .badge :global(svg) { color: var(--muted); }
  .badge.warn :global(svg) { color: var(--warn); }
  .badge:hover, .badge[aria-expanded="true"] { background: var(--surface-3); color: var(--text); }
  .badge:focus-visible { outline: 2px solid var(--accent); }
  .popover {
    position: absolute; top: calc(100% + 6px); left: 0; z-index: 30; display: flex; flex-direction: column; gap: 8px;
    width: max-content; max-width: min(420px, calc(100vw - 32px)); padding: 10px 12px;
    border-radius: var(--radius); background: var(--surface-2); box-shadow: var(--popover-shadow);
    color: var(--text-2); font-size: 12.5px; line-height: 1.45; white-space: normal; animation: appear 0.12s ease-out;
  }
  .popover[hidden] { display: none; }
  @keyframes appear { from { opacity: 0; transform: translateY(-4px); } }
  .note { display: flex; flex-direction: column; align-items: flex-start; gap: 2px; padding-left: 8px; border-left: 2px solid var(--border-strong); }
  .note.warn { border-left-color: var(--warn); }
  .link { border: 0; padding: 0; background: none; font: inherit; color: var(--accent-strong); cursor: pointer; }
  .link:hover { text-decoration: underline; }
</style>
