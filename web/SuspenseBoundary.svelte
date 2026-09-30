<script lang="ts">
  import type { Snippet } from "svelte";
  // The model stages replacements atomically, retaining the resolved content
  // during subsequent requests. Only an unresolved initial load needs fallback.
  let { ready, children, pending }: { ready: boolean; children: Snippet; pending: Snippet } = $props();
</script>

{#if ready}
  {@render children()}
{:else}
  {@render pending()}
{/if}
