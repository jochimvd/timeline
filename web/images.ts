import type { Shot } from "./types";

const PIXEL_LIMIT = 96 * 1024 * 1024;
const BUNDLE_LIMIT = 32 * 1024 * 1024;
const HEADER_LIMIT = 4 * 1024 * 1024;
type Tile = { key: string; bitmap: ImageBitmap; bytes: number; refs: number };
type Header = {
  version: number; width: number; height: number; tile_size: number;
  tiles: { key: string; width: number; height: number; length: number }[];
  map: number[];
};
type Asset = { key: string; header: Header; tiles: Tile[]; bytes: number; refs: number; draw?: ImageFrame["draw"] };
type Pool = { frames: Map<string, Asset>; tiles: Map<string, Tile>; decodedBytes: number; canvasBytes: number };
export type ImageFrame = { shot: Shot; draw?: (canvas: HTMLCanvasElement, maxWidth?: number) => () => void; error?: string };
export const createImagePool = (): Pool => ({ frames: new Map(), tiles: new Map(), decodedBytes: 0, canvasBytes: 0 });
const integer = (n: unknown): n is number => Number.isSafeInteger(n) && (n as number) > 0;
function previewDimensions(width: number, height: number) {
  const edge = Math.max(width, height), scale = Math.min(640, edge);
  return { width: Math.max(1, Math.round(width * scale / edge)), height: Math.max(1, Math.round(height * scale / edge)) };
}

function parseBundle(bytes: Uint8Array<ArrayBuffer>, width: number, height: number): { header: Header; offset: number } {
  if (bytes.length < 4) throw new Error("Truncated tile bundle");
  const length = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(0, true);
  if (length > HEADER_LIMIT || length + 4 > bytes.length) throw new Error("Invalid tile bundle header");
  const h = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(4, length + 4))) as Header;
  if (!h || h.version !== 1 || h.width !== width || h.height !== height || !integer(h.tile_size)
    || h.tile_size > 16383 || !Array.isArray(h.tiles) || !Array.isArray(h.map)) throw new Error("Invalid tile dimensions");
  const columns = Math.ceil(width / h.tile_size), rows = Math.ceil(height / h.tile_size);
  if (h.map.length !== columns * rows || h.map.length > 4096 || !h.tiles.length || h.tiles.length > h.map.length)
    throw new Error("Invalid tile map");
  let end = length + 4;
  const keys = new Set<string>(), used = new Set<number>();
  for (const t of h.tiles) {
    if (!t || typeof t.key !== "string" || !t.key.length || t.key.length > 256 || keys.has(t.key)
      || !integer(t.width) || !integer(t.height) || t.width > h.tile_size || t.height > h.tile_size
      || !integer(t.length) || t.length > BUNDLE_LIMIT) throw new Error("Invalid tile asset");
    keys.add(t.key);
    end += t.length;
  }
  if (end !== bytes.length) throw new Error("Truncated or trailing tile payload");
  h.map.forEach((index, position) => {
    if (!Number.isSafeInteger(index) || index < 0 || index >= h.tiles.length) throw new Error("Invalid tile reference");
    const tile = h.tiles[index];
    const x = position % columns * h.tile_size, y = Math.floor(position / columns) * h.tile_size;
    if (tile.width !== Math.min(h.tile_size, width - x) || tile.height !== Math.min(h.tile_size, height - y))
      throw new Error("Tile does not fit screenshot");
    used.add(index);
  });
  if (used.size !== h.tiles.length) throw new Error("Unreferenced tile asset");
  return { header: h, offset: length + 4 };
}

/** Only active selected/hover frames and their pending replacements retain tiles. */
export function createImageLoader(pool = createImagePool(), thumbnail = false) {
  let current = new Map<string, Asset>();
  function releaseTile(tile: Tile) {
    if (--tile.refs === 0) {
      tile.bitmap.close();
      pool.decodedBytes -= tile.bytes;
      if (pool.tiles.get(tile.key) === tile) pool.tiles.delete(tile.key);
    }
  }
  function release(asset: Asset) {
    if (--asset.refs === 0) {
      asset.tiles.forEach(releaseTile);
      if (pool.frames.get(asset.key) === asset) pool.frames.delete(asset.key);
    }
  }
  function clear() { current.forEach(release); current.clear(); }

  async function retrieve(id: number, signal: AbortSignal) {
    let response: Response;
    for (let attempt = 0; ; attempt++) {
      response = await fetch((thumbnail ? "/api/thumbnail/" : "/api/tiles/") + id, { signal });
      if (response.status !== 503 || attempt === 8) break;
      await new Promise(resolve => setTimeout(resolve, 75));
      signal.throwIfAborted();
    }
    if (!response.ok) throw new Error("Image unavailable (" + response.status + "); no replacement shown.");
    const reader = response.body!.getReader();
    const chunks: Uint8Array<ArrayBuffer>[] = [];
    let length = 0;
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        length += value.byteLength;
        if (length > (thumbnail ? 1024 * 1024 : BUNDLE_LIMIT)) {
          await reader.cancel();
          throw new Error("Display image byte limit. Select one monitor.");
        }
        chunks.push(value);
      }
    } finally { reader.releaseLock(); }
    signal.throwIfAborted();
    const bytes = new Uint8Array(length);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    return bytes;
  }

  async function acquire(key: string, shot: Shot, signal: AbortSignal): Promise<Asset> {
    let existing = pool.frames.get(key);
    if (existing) { existing.refs++; return existing; }
    const bytes = await retrieve(shot.screenshot.id, signal);
    const source = shot.screenshot;
    const { width, height } = previewDimensions(source.width, source.height);
    const parsed = thumbnail ? {
      offset: 0,
      header: { version: 1, width, height, tile_size: Math.max(width, height),
        tiles: [{ key, width, height, length: bytes.length }], map: [0] } satisfies Header,
    } : parseBundle(bytes, source.width, source.height);
    const tiles: Tile[] = [];
    let offset = parsed.offset;
    try {
      for (const entry of parsed.header.tiles) {
        signal.throwIfAborted();
        let tile = pool.tiles.get(entry.key);
        if (!tile) {
          const size = entry.width * entry.height * 4;
          if (pool.decodedBytes + size > PIXEL_LIMIT) throw new Error("Display tile memory limit. Select one monitor.");
          pool.decodedBytes += size;
          let bitmap: ImageBitmap | undefined;
          try {
            bitmap = await createImageBitmap(new Blob([bytes.subarray(offset, offset + entry.length)], { type: "image/webp" }));
            signal.throwIfAborted();
            if (bitmap.width !== entry.width || bitmap.height !== entry.height) throw new Error("Decoded tile dimensions do not match");
            // Another loader can finish the same tile while decode is pending.
            tile = pool.tiles.get(entry.key);
            if (!tile) {
              tile = { key: entry.key, bitmap, bytes: size, refs: 0 };
              pool.tiles.set(entry.key, tile);
              bitmap = undefined;
            } else {
              bitmap.close(); bitmap = undefined;
              pool.decodedBytes -= size;
            }
          } catch (error) {
            bitmap?.close();
            pool.decodedBytes -= size;
            throw error;
          }
        }
        if (tile.bitmap.width !== entry.width || tile.bitmap.height !== entry.height) throw new Error("Conflicting tile dimensions");
        tile.refs++;
        tiles.push(tile);
        offset += entry.length;
      }
      signal.throwIfAborted();
      existing = pool.frames.get(key);
      if (existing) {
        tiles.forEach(releaseTile);
        existing.refs++;
        return existing;
      }
      const asset = { key, header: parsed.header, tiles, bytes: bytes.length, refs: 1 };
      pool.frames.set(key, asset);
      return asset;
    } catch (error) { tiles.forEach(releaseTile); throw error; }
  }

  function drawing(asset: Asset) {
    return (canvas: HTMLCanvasElement, maxWidth?: number) => {
      const h = asset.header;
      const width = integer(maxWidth) ? Math.min(h.width, maxWidth) : h.width;
      const height = Math.max(1, Math.round(h.height * width / h.width));
      const bytes = width * height * 4;
      if (pool.canvasBytes + bytes > PIXEL_LIMIT) throw new Error("Display canvas memory limit. Close the preview or select one monitor.");
      pool.canvasBytes += bytes;
      let released = false;
      const dispose = () => {
        if (!released) { released = true; canvas.width = 0; canvas.height = 0; pool.canvasBytes -= bytes; }
      };
      try {
        canvas.width = width; canvas.height = height;
        const context = canvas.getContext("2d", { alpha: false });
        if (!context) throw new Error("Canvas rendering unavailable");
        const columns = Math.ceil(h.width / h.tile_size);
        h.map.forEach((index, position) => {
          const tile = asset.tiles[index].bitmap;
          const x = position % columns * h.tile_size, y = Math.floor(position / columns) * h.tile_size;
          if (width === h.width) context.drawImage(tile, x, y);
          else context.drawImage(tile, x * width / h.width, y * height / h.height,
            tile.width * width / h.width, tile.height * height / h.height);
        });
      } catch (error) { dispose(); throw error; }
      return dispose;
    };
  }

  async function load(shots: Shot[], layout: string, signal: AbortSignal): Promise<ImageFrame[]> {
    const next = new Map<string, Asset>();
    const frames: ImageFrame[] = [];
    let decodedBytes = 0, encodedBytes = 0, committed = false;
    try {
      for (const shot of shots.filter(s => !layout.startsWith("monitor:") || s.screenshot.monitor === layout.slice(8))) {
        signal.throwIfAborted();
        const s = shot.screenshot, frame: ImageFrame = { shot };
        frames.push(frame);
        if (s.unavailable) { frame.error = "Unavailable: " + s.unavailable; continue; }
        const dimensions = thumbnail ? previewDimensions(s.width, s.height) : s;
        const pixelBytes = dimensions.width * dimensions.height * 4;
        if (!integer(s.width) || !integer(s.height) || s.width > 16383 || s.height > 16383
          || decodedBytes + pixelBytes > PIXEL_LIMIT) {
          frame.error = "Display memory limit. Select this monitor individually."; continue;
        }
        decodedBytes += pixelBytes;
        try {
          const key = (thumbnail ? "thumbnail:" : "") + (s.image ? "image:" + s.image : "capture:" + s.id);
          let asset = next.get(key);
          if (!asset) {
            asset = await acquire(key, shot, signal);
            if (encodedBytes + asset.bytes > BUNDLE_LIMIT) {
              release(asset);
              throw new Error("Display image byte limit. Select one monitor.");
            }
            next.set(key, asset);
          }
          if (encodedBytes + asset.bytes > BUNDLE_LIMIT) throw new Error("Display image byte limit. Select one monitor.");
          encodedBytes += asset.bytes;
          // Capture IDs change even when the encoded image is identical. Keep
          // its drawing function stable so mounted canvases can keep their pixels.
          frame.draw = asset.draw ??= drawing(asset);
        } catch (error) {
          signal.throwIfAborted();
          frame.error = error instanceof Error ? error.message : "Image unavailable";
        }
      }
      signal.throwIfAborted();
      current.forEach(release);
      current = next;
      committed = true;
      return frames;
    } finally { if (!committed) next.forEach(release); }
  }
  return { load, clear };
}
