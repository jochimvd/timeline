import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";

const built = await build({ entryPoints: ["web/images.ts"], bundle: true, write: false, format: "esm", platform: "browser" });
const { createImageLoader, createImagePool } = await import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));
const shot = (id, image, width = 4, height = 3) => ({ screenshot: { id, image, width, height, monitor: "A", unavailable: null }, stale: false });
function bundle(keys = ["a", "b", "c", "d"]) {
  const header = { version: 1, width: 4, height: 3, tile_size: 2,
    tiles: keys.map((key, i) => ({ key, width: 2, height: i < 2 ? 2 : 1, length: 2 })), map: [0, 1, 2, 3] };
  const json = new TextEncoder().encode(JSON.stringify(header));
  const bytes = new Uint8Array(4 + json.length + 8);
  new DataView(bytes.buffer).setUint32(0, json.length, true);
  bytes.set(json, 4);
  keys.forEach((_, i) => bytes.set([2, i < 2 ? 2 : 1], 4 + json.length + 2 * i));
  return bytes;
}
function fixture() {
  const originalFetch = globalThis.fetch, originalBitmap = globalThis.createImageBitmap;
  const decoded = [], requests = [];
  globalThis.fetch = async url => { requests.push(url); return new Response(bundle(url.endsWith("/3") ? ["a", "new", "c", "d"] : undefined)); };
  globalThis.createImageBitmap = async blob => {
    const bytes = new Uint8Array(await blob.arrayBuffer());
    const bitmap = { width: bytes[0], height: bytes[1], closed: false, close() { assert.equal(this.closed, false); this.closed = true; } };
    decoded.push(bitmap); return bitmap;
  };
  return { decoded, requests, restore() { globalThis.fetch = originalFetch; globalThis.createImageBitmap = originalBitmap; } };
}

test("active selected and hover frames reuse decoded tiles and free all surfaces", async () => {
  const f = fixture(), pool = createImagePool(), selected = createImageLoader(pool), hover = createImageLoader(pool);
  const signal = new AbortController().signal;
  try {
    const [frame] = await selected.load([shot(1, "first")], "grid", signal);
    assert.equal(f.decoded.length, 4);
    const calls = [], canvas = { width: 0, height: 0, getContext: () => ({ drawImage: (...args) => calls.push(args) }) };
    const dispose = frame.draw(canvas);
    assert.deepEqual(calls.map(call => call.slice(1)), [[0, 0], [2, 0], [0, 2], [2, 2]]);
    assert.equal(pool.canvasBytes, 48);
    const [sameContent] = await hover.load([shot(2, "first")], "grid", signal);
    assert.equal(sameContent.draw, frame.draw, "unchanged content keeps its drawing function and canvas pixels");
    assert.equal(f.requests.length, 1, "same screenshot content avoids retrieval");
    await selected.load([shot(3, "second")], "grid", signal);
    assert.equal(f.decoded.length, 5, "only the changed tile is decoded");
    selected.clear();
    assert.equal(f.decoded.filter(bitmap => bitmap.closed).length, 1);
    hover.clear(); dispose(); dispose();
    assert.ok(f.decoded.every(bitmap => bitmap.closed));
    assert.equal(pool.decodedBytes, 0); assert.equal(pool.canvasBytes, 0);
    assert.equal(pool.tiles.size, 0); assert.equal(pool.frames.size, 0);
  } finally { selected.clear(); hover.clear(); f.restore(); }
});

test("malformed tile bundles fail before any bitmap decoding", async () => {
  const f = fixture(), pool = createImagePool(), loader = createImageLoader(pool);
  try {
    const bytes = bundle();
    globalThis.fetch = async () => new Response(bytes.subarray(0, bytes.length - 1));
    const [frame] = await loader.load([shot(1, "bad")], "grid", new AbortController().signal);
    assert.match(frame.error, /Truncated/);
    assert.equal(f.decoded.length, 0); assert.equal(pool.decodedBytes, 0);
  } finally { loader.clear(); f.restore(); }
});

test("cancelled bitmap decoding closes pending tiles and leaves the displayed frame intact", async () => {
  const f = fixture(), pool = createImagePool(), loader = createImageLoader(pool), controller = new AbortController();
  try {
    await loader.load([shot(1, "first")], "grid", new AbortController().signal);
    const decode = globalThis.createImageBitmap;
    globalThis.createImageBitmap = async blob => { const bitmap = await decode(blob); controller.abort(); return bitmap; };
    await assert.rejects(loader.load([shot(3, "second")], "grid", controller.signal), { name: "AbortError" });
    assert.equal(pool.frames.size, 1); assert.equal(pool.tiles.size, 4);
    assert.equal(f.decoded.at(-1).closed, true);
    assert.ok(f.decoded.slice(0, 4).every(bitmap => !bitmap.closed));
    loader.clear(); assert.equal(pool.decodedBytes, 0);
  } finally { loader.clear(); f.restore(); }
});

test("shared canvas budget rejects oversized concurrent surfaces and recovers on disposal", async () => {
  const f = fixture(), pool = createImagePool(), loader = createImageLoader(pool);
  try {
    const [frame] = await loader.load([shot(1, "first")], "grid", new AbortController().signal);
    const canvas = { width: 0, height: 0, getContext: () => ({ drawImage() {} }) };
    pool.canvasBytes = 96 * 1024 * 1024;
    assert.throws(() => frame.draw(canvas), /memory limit/);
    pool.canvasBytes = 0;
    const dispose = frame.draw(canvas); dispose();
    assert.equal(pool.canvasBytes, 0); assert.equal(canvas.width, 0);
    const thumbnail = frame.draw(canvas, 2);
    assert.equal(canvas.width, 2); assert.equal(canvas.height, 2);
    assert.equal(pool.canvasBytes, 16, "preview canvases reserve their thumbnail pixels, not full screenshot memory");
    thumbnail(); assert.equal(pool.canvasBytes, 0);
  } finally { loader.clear(); f.restore(); }
});

test("Chromium decodes and assembles actual WebP edge tiles without re-encoding the screenshot", async () => {
  const { chromium } = await import("@playwright/test");
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    const result = await page.evaluate(async source => {
      const { createImagePool, createImageLoader } = await import("data:text/javascript;base64," + btoa(source));
      const entries = [], payloads = [];
      for (const [index, color] of ["red", "lime", "blue", "white"].entries()) {
        const tile = document.createElement("canvas"); tile.width = 2; tile.height = index < 2 ? 2 : 1;
        const context = tile.getContext("2d"); context.fillStyle = color; context.fillRect(0, 0, tile.width, tile.height);
        const blob = await new Promise(resolve => tile.toBlob(resolve, "image/webp", 1));
        const bytes = new Uint8Array(await blob.arrayBuffer());
        entries.push({ key: color, width: tile.width, height: tile.height, length: bytes.length }); payloads.push(bytes);
      }
      const json = new TextEncoder().encode(JSON.stringify({ version: 1, width: 4, height: 3, tile_size: 2, tiles: entries, map: [0, 1, 2, 3] }));
      const bundle = new Uint8Array(4 + json.length + payloads.reduce((total, bytes) => total + bytes.length, 0));
      new DataView(bundle.buffer).setUint32(0, json.length, true); bundle.set(json, 4);
      let offset = 4 + json.length;
      for (const bytes of payloads) { bundle.set(bytes, offset); offset += bytes.length; }
      window.fetch = async () => new Response(bundle);
      const pool = createImagePool(), loader = createImageLoader(pool);
      const [frame] = await loader.load([{ screenshot: { id: 1, image: "tiles", monitor: "A", width: 4, height: 3, unavailable: null } }], "grid", new AbortController().signal);
      const canvas = document.createElement("canvas"), dispose = frame.draw(canvas);
      const context = canvas.getContext("2d");
      const colors = [[0, 0], [3, 0], [0, 2], [3, 2]].map(([x, y]) => Array.from(context.getImageData(x, y, 1, 1).data));
      dispose(); loader.clear();
      return { colors, decodedBytes: pool.decodedBytes, canvasBytes: pool.canvasBytes };
    }, built.outputFiles[0].text);
    assert.ok(result.colors[0][0] > 240 && result.colors[0][1] < 15 && result.colors[0][2] < 15);
    assert.ok(result.colors[1][0] < 15 && result.colors[1][1] > 240 && result.colors[1][2] < 15);
    assert.ok(result.colors[2][0] < 15 && result.colors[2][1] < 15 && result.colors[2][2] > 240);
    assert.ok(result.colors[3].every(value => value > 240));
    assert.equal(result.decodedBytes, 0); assert.equal(result.canvasBytes, 0);
  } finally { await browser.close(); }
});

test("a missing selected screenshot releases old tiles and returns explicit unavailable evidence", async () => {
  const f = fixture(), pool = createImagePool(), loader = createImageLoader(pool);
  try {
    await loader.load([shot(1, "first")], "grid", new AbortController().signal);
    globalThis.fetch = async () => new Response(null, { status: 404 });
    const [frame] = await loader.load([shot(2, "missing")], "grid", new AbortController().signal);
    assert.match(frame.error, /Image unavailable \(404\)/); assert.equal(frame.draw, undefined);
    assert.equal(pool.decodedBytes, 0); assert.equal(pool.frames.size, 0); assert.equal(pool.tiles.size, 0);
    assert.ok(f.decoded.every(bitmap => bitmap.closed));
  } finally { loader.clear(); f.restore(); }
});

test("hover uses small previews, shares unchanged content, and releases its bitmap", async () => {
  const f = fixture(), pool = createImagePool(), loader = createImageLoader(pool, true);
  const urls = [];
  globalThis.fetch = async url => { urls.push(url); return new Response(new Uint8Array([1])); };
  globalThis.createImageBitmap = async () => {
    const bitmap = { width: 640, height: 360, closed: false, close() { this.closed = true; } };
    f.decoded.push(bitmap); return bitmap;
  };
  try {
    const [first] = await loader.load([shot(1, "large", 3840, 2160)], "grid", new AbortController().signal);
    assert.equal(typeof first.draw, "function");
    assert.equal(pool.decodedBytes, 640 * 360 * 4);
    assert.deepEqual(urls, ["/api/thumbnail/1"]);
    const [same] = await loader.load([shot(2, "large", 3840, 2160)], "grid", new AbortController().signal);
    assert.equal(same.draw, first.draw);
    assert.equal(urls.length, 1);
    loader.clear();
    assert.equal(pool.decodedBytes, 0);
    assert.ok(f.decoded[0].closed);
  } finally { loader.clear(); f.restore(); }
});
