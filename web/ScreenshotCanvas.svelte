<script lang="ts">
  import type { ImageFrame } from "./images";
  let { frame, label, transform, maxWidth }: { frame: ImageFrame; label: string; transform: string; maxWidth?: number } = $props();
  let error = $state<string | null>(null);
  type Rendering = { draw: ImageFrame["draw"]; maxWidth?: number };
  function render(canvas: HTMLCanvasElement, rendering: Rendering) {
    let dispose: (() => void) | undefined;
    let painted: ImageFrame["draw"];
    let paintedWidth: number | undefined;
    function update({ draw, maxWidth }: Rendering) {
      // Metadata can change while the pixels are identical. Svelte still calls
      // action.update for the new frame, so avoid clearing this canvas again.
      if (dispose && draw === painted && maxWidth === paintedWidth) return;
      dispose?.(); dispose = undefined; error = null;
      painted = undefined;
      try { dispose = draw?.(canvas, maxWidth); painted = draw; paintedWidth = maxWidth; }
      catch (cause) { error = cause instanceof Error ? cause.message : "Image rendering failed"; }
    }
    update(rendering);
    return { update, destroy: () => dispose?.() };
  }
</script>
<canvas use:render={{ draw: frame.draw, maxWidth }} aria-label={label} style:transform style:display={error ? "none" : undefined}></canvas>
{#if error}<span class="error">{error}</span>{/if}
