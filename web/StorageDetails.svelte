<script lang="ts">
  import { onDestroy } from "svelte";
  import { request } from "./api";
  type Storage = { total_bytes: number; pack_bytes: number; pack_count: number; database_bytes: number;
    map_bytes: number; map_uncompressed_bytes: number; captures: number; images: number; regions: number;
    tile_reuse_percent: number | null; thumbnail_cache_bytes: number; thumbnail_cache_limit: number;
    retention_days: number; measured_ms: number };
  let data = $state<Storage>();
  let busy = $state(false), error = $state("");
  let controller: AbortController | undefined;
  function bytes(value: number | undefined) {
    if (value === undefined) return "—";
    const unit = value >= 1024 ** 3 ? 3 : value >= 1024 ** 2 ? 2 : value >= 1024 ? 1 : 0;
    return `${(value / 1024 ** unit).toFixed(unit ? 1 : 0)} ${["B", "KiB", "MiB", "GiB"][unit]}`;
  }
  async function refresh() {
    if (busy) return;
    const current = controller = new AbortController();
    busy = true; error = "";
    try { data = await request<Storage>("/api/storage", undefined, current.signal); }
    catch (e) { if (!current.signal.aborted) error = e instanceof Error ? e.message : "Storage details unavailable"; }
    finally { if (controller === current) busy = false; }
  }
  function toggle(event: Event) {
    if ((event.currentTarget as HTMLDetailsElement).open) {
      if (!data || Date.now() - data.measured_ms > 30000) void refresh();
    } else { controller?.abort(); controller = undefined; busy = false; }
  }
  onDestroy(() => controller?.abort());
</script>

<details class="page-details storage-details" ontoggle={toggle}>
  <summary>Storage details</summary>
  <div class="storage-body" aria-busy={busy}>
    <dl>
      <div><dt>Images + database</dt><dd>{bytes(data?.total_bytes)}</dd></div>
      <div><dt>Image packs</dt><dd>{bytes(data?.pack_bytes)}</dd></div>
      <div><dt>Database + journal</dt><dd>{bytes(data?.database_bytes)}</dd></div>
      <div><dt>Tile maps <small>included in database</small></dt><dd>{bytes(data?.map_bytes)}</dd></div>
      <div><dt>Tile reuse <small>bytes saved across tiled images</small></dt><dd>{data?.tile_reuse_percent == null ? "—" : `${data.tile_reuse_percent.toFixed(1)}%`}</dd></div>
      <div><dt>Preview cache <small>RAM only{data ? ` · ${bytes(data.thumbnail_cache_limit)} limit` : ""}</small></dt><dd>{bytes(data?.thumbnail_cache_bytes)}</dd></div>
    </dl>
    <div class="storage-caption">
      <span>{data ? `${data.captures.toLocaleString()} captures · ${data.regions.toLocaleString()} regions · ${data.retention_days}-day retention` : "\u00a0"}</span>
      <button onclick={() => void refresh()} disabled={busy}>Refresh</button>
    </div>
    <p class="storage-error" role="status">{error || "\u00a0"}</p>
  </div>
</details>
