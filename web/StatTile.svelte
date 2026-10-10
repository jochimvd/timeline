<script lang="ts">
  import type { Snippet } from "svelte";
  import type { Delta } from "./trends";
  let { id, label, children, detail, delta, action }: {
    id?: string; label: string; children: Snippet; detail?: Snippet; delta?: Delta | string; action?: Snippet;
  } = $props();
</script>

<!-- One figure of a `dl.stat-tiles` row: label, value, optional change, detail and action.
     A change shows its direction with an arrow in text colors: it is observed, not judged. -->
<div class="stat-tile" {id}>
  <dt>{label}</dt>
  <dd class="stat-value">{@render children()}</dd>
  {#if typeof delta === "string"}
    <dd class="stat-delta muted">{delta}</dd>
  {:else if delta}
    <dd class="stat-delta"><span aria-hidden="true">{delta.direction > 0 ? "▲" : delta.direction < 0 ? "▼" : "="}</span>
      {delta.text} <span class="muted">{delta.label}</span></dd>
  {/if}
  {#if detail}<dd class="stat-detail">{@render detail()}</dd>{/if}
  {#if action}<dd class="stat-action">{@render action()}</dd>{/if}
</div>
