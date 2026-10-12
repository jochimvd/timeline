<script lang="ts">
  import Icon from "./Icon.svelte";
  import { dismiss, toasts } from "./toasts";
</script>

<!-- Floats over the page, so feedback never moves its layout. -->
<div id="toasts" class="toasts" role="status" aria-live="polite">
  {#each $toasts as toast (toast.id)}
    <div class="toast" class:error={toast.tone === "error"}>
      <Icon name={toast.tone === "error" ? "alert" : "info"} size={14} />
      <span class="text">{toast.text}</span>
      {#if toast.retry}
        {@const retry = toast.retry}
        <button class="btn small ghost" onclick={() => { dismiss(toast.key); retry(); }}>Retry</button>
      {/if}
      {#if toast.tone === "error"}
        <button class="btn small icon ghost" aria-label="Dismiss" title="Dismiss" onclick={() => dismiss(toast.key)}><Icon name="x" size={14} /></button>
      {/if}
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed; left: 50%; bottom: 20px; z-index: 40; transform: translateX(-50%);
    display: flex; flex-direction: column; align-items: center; gap: 8px; pointer-events: none;
  }
  .toast {
    display: flex; align-items: center; gap: 8px; max-width: min(560px, calc(100vw - 32px)); padding: 8px 12px;
    border-radius: var(--radius); background: var(--surface-2); border: 1px solid var(--border); box-shadow: var(--popover-shadow);
    color: var(--text-2); font-size: 13px; pointer-events: auto; animation: rise 0.15s ease-out;
  }
  .toast:has(button) { padding-right: 4px; }
  .toast > :global(svg) { flex: none; color: var(--muted); }
  .toast.error > :global(svg) { color: var(--bad); }
  .text { overflow-wrap: anywhere; }
  @keyframes rise { from { opacity: 0; transform: translateY(6px); } }
</style>
