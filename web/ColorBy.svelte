<script lang="ts">
  import { colorBy } from "./preferences";
  /** Offer subcategories only when some category is nested. */
  let { nested = false }: { nested?: boolean } = $props();
</script>

<!-- Shared by the day and period views; shown only with configured categories. -->
<fieldset class="segmented" id="color-by">
  <legend>Color by</legend>
  <div class="segmented-options">
    <label><input type="radio" name="color-by" value="application" bind:group={$colorBy} />Application</label>
    <label><input type="radio" name="color-by" value="category" bind:group={$colorBy} />Category</label>
    {#if nested}<label><input type="radio" name="color-by" value="subcategory" bind:group={$colorBy} />Subcategory</label>{/if}
  </div>
</fieldset>
