<script lang="ts">
  import type { Snippet } from "svelte";
  /** Floats above the hovered or focused mark (below it near the top), or beside
   *  it, so a tall column's tooltip does not cover the legend. */
  let { anchor, id, side = false, children }: { anchor: DOMRect; id: string; side?: boolean; children: Snippet } = $props();
  let width = $state(240), height = $state(80), viewport = $state(1024), viewportHeight = $state(768);
  const left = $derived(side
    ? (anchor.right + 12 + width <= viewport - 8 ? anchor.right + 12 : Math.max(8, anchor.left - 12 - width))
    : Math.max(8, Math.min(anchor.left + anchor.width / 2 - width / 2, viewport - width - 8)));
  const top = $derived(side ? Math.max(8, Math.min(anchor.top, viewportHeight - height - 8))
    : anchor.top - height - 8 >= 8 ? anchor.top - height - 8 : anchor.bottom + 8);
</script>

<svelte:window bind:innerWidth={viewport} bind:innerHeight={viewportHeight} />
<div class="chart-tooltip" role="tooltip" {id} bind:clientWidth={width} bind:clientHeight={height}
  style:left={left + "px"} style:top={top + "px"}>{@render children()}</div>
