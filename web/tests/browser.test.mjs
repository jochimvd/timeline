import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { chromium } from "@playwright/test";

/** The fixture embeds web assets at compile time. Cargo can consider a fixture
 *  built from other sources fresh (for example after building another checkout
 *  into the same target directory), which would test an old UI; fail clearly. */
async function assertServesCheckout(url) {
  for (const asset of ["app.js", "style.css"]) {
    const served = await (await fetch(new URL("/" + asset, url))).text();
    assert.ok(served === await readFile("web/" + asset, "utf8"),
      `the fixture serves a stale web/${asset}; rebuild it with \`touch web/${asset} && cargo build --example web_fixture\``);
  }
}

/** Starts the synthetic fixture: its process, output lines and launch URL. */
async function startFixture() {
  const fixture = spawn("cargo", ["run", "--quiet", "--example", "web_fixture"], { stdio: ["pipe", "pipe", "inherit"] });
  const lines = createInterface({ input: fixture.stdout });
  const url = await new Promise((resolve, reject) => {
    lines.once("line", resolve);
    fixture.once("exit", (code) => reject(new Error(`Fixture exited ${code}`)));
  });
  await assertServesCheckout(url);
  return { fixture, lines, url };
}
async function stopFixture(fixture) {
  fixture.stdin.end("\n");
  if (fixture.exitCode === null) await new Promise((resolve) => fixture.once("exit", resolve));
}

test("checked-in JavaScript matches Svelte and TypeScript", async () => {
  const dir = await mkdtemp(join(tmpdir(), "timeline-ts-"));
  try {
    execFileSync(process.execPath, ["web/build.mjs", dir]);
    assert.equal(
      await readFile(join(dir, "app.js"), "utf8"),
      await readFile("web/app.js", "utf8"),
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});


/** 2026-09-22 04:00 Europe/Oslo: the fixture's activity day, with an hour of focus from here. */
const BASE = 1790042400000;
/** The page x of a time on the timeline, from its visible range. */
const xAt = (page, ms) => page.evaluate(ms => {
  const timeline = document.querySelector("#timeline"), rect = timeline.getBoundingClientRect();
  const start = Number(timeline.getAttribute("aria-valuemin")), end = Number(timeline.getAttribute("aria-valuemax"));
  return rect.left + rect.width * (ms - start) / (end - start);
}, ms);
/** A y inside the timeline's focus band. */
const bandY = async page => (await page.locator("#timeline .band").boundingBox()).y + 20;
const timelineIdle = page => page.waitForFunction(() => document.querySelector("#timeline")?.getAttribute("aria-busy") === "false");
const imagesIdle = page => page.waitForFunction(() => document.querySelector("#images")?.getAttribute("aria-busy") === "false");
async function chooseDate(page, date) {
  await page.locator("#date").fill(date);
  await page.locator("#date").dispatchEvent("change");
}

test("day view: screenshots, timeline, sessions, breakdown, monitors and obsolete responses", async () => {
  const { fixture, lines, url } = await startFixture();
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
    const page = await context.newPage();
    const errors = [];
    await page.addInitScript(() => {
      window.__timelineFormatters = 0;
      Intl.DateTimeFormat = new Proxy(Intl.DateTimeFormat, {
        construct(target, args) {
          window.__timelineFormatters++;
          return Reflect.construct(target, args);
        },
      });
    });
    page.on("pageerror", (e) => errors.push(e.message));
    // Install the clock before interactions so later date changes do not reset performance.now().
    await page.clock.setFixedTime(new Date("2026-09-23T12:00:00Z"));
    await page.goto(url);
    await page.waitForFunction(() => document.querySelectorAll("#images canvas").length === 2
      && document.querySelector("#images").getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#date").inputValue(), "2026-09-22", "opening finds captures from an earlier activity day");
    assert.match(await page.locator("#selected").innerText(), /04:09:55/, "opening selects the latest available screenshot");
    assert.equal(await page.locator("#zoom").inputValue(), "fit", "a day opens framed on its recorded activity");
    await timelineIdle(page);

    // Whole-day figures and the day's sessions.
    await page.waitForFunction(() => document.querySelector("#sessions"));
    assert.match(await page.locator("#day-totals").innerText(), /Focus\s+1h\s+1h active · 0 min idle/);
    assert.match(await page.locator("#day-totals").innerText(), /Switches\s+0\/h/);
    assert.match(await page.locator("#day-totals").innerText(), /Longest stretch\s+1h\s+Planning · 04:00/);
    assert.match(await page.locator("#day-totals").innerText(), /Deep focus\s+100%/);
    assert.equal(await page.locator("#sessions > li").count(), 1);
    assert.match(await page.locator("#sessions").innerText(), /04:00\s+Planning\s+1h\s+E\s+<script>Planning notes<\/script>/);
    assert.equal(await page.locator("#sessions script").count(), 0);

    // Storage details are read when shown, and a recent snapshot is reused.
    let storageRequests = 0;
    page.on("request", request => { if (request.url().endsWith("/api/storage")) storageRequests++; });
    assert.equal(storageRequests, 0);
    await page.getByRole("button", { name: "Settings and details" }).click();
    await page.locator(".drawer nav button", { hasText: "Storage" }).click();
    await page.waitForFunction(() => document.querySelectorAll(".drawer .stats dd").length === 6
      && ![...document.querySelectorAll(".drawer .stats dd")].some(dd => dd.textContent === "—"));
    assert.equal(storageRequests, 1);
    await page.locator(".drawer nav button", { hasText: "Categories" }).click();
    await page.waitForFunction(() => document.querySelectorAll("#category-rules > li").length === 4);
    assert.match(await page.locator("#category-rules").innerText(), /Research[\s\S]*Browser[\s\S]*Mail/);
    await page.locator(".drawer nav button", { hasText: "Storage" }).click();
    await page.keyboard.press("Escape");
    assert.equal(await page.locator(".drawer").evaluate(d => d.open), false);
    await page.getByRole("button", { name: "Settings and details" }).click();
    await page.locator(".drawer nav button", { hasText: "Storage" }).click();
    assert.equal(storageRequests, 1, "opening a recent storage snapshot should not fetch again");
    await page.keyboard.press("Escape");

    const emptyPage = await page.context().newPage();
    await emptyPage.clock.setFixedTime(new Date("2026-09-23T12:00:00Z"));
    await emptyPage.route("**/api/query/adjacent_screenshot", route => route.fulfill({ json: null }));
    await emptyPage.goto(new URL("/", url).href);
    await emptyPage.waitForFunction(() => document.querySelector("#date").value === "2026-09-23"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.equal(await emptyPage.locator("#images canvas").count(), 0, "no captures leaves an unselected viewer on today's activity day");
    assert.match(await emptyPage.locator("#images").innerText(), /Pick a moment/);
    await emptyPage.close();

    // Screenshot coverage keeps even subpixel gaps.
    const coverage = await page.locator('#timeline .shots[title="Screenshots · Display A"] > div')
      .evaluateAll(nodes => nodes.map(node => node.classList.contains("shot-gap") ? "shot-gap" : "shot"));
    assert.equal(coverage[0], "shot");
    assert.ok(coverage.includes("shot-gap"), "aggregation must retain even subpixel screenshot gaps");
    assert.equal(coverage.at(-1), "shot");

    const timeline = page.locator("#timeline");
    const y = await bandY(page);
    // Clicks land on whole pixels, seconds apart at this scale: stay clear of the first capture.
    await page.mouse.click(await xAt(page, BASE + 20_000), y);
    await page.waitForFunction(() => document.querySelectorAll("#images canvas").length === 2
      && document.querySelector("#images").getAttribute("aria-busy") !== "true");
    let repeatedImageRequests = 0;
    const countImages = (request) => { if (request.url().includes("/api/tiles/")) repeatedImageRequests++; };
    page.on("request", countImages);
    const selectedFirst = await page.locator("#selected").innerText();
    await timeline.dispatchEvent("click", { clientX: await xAt(page, BASE + 20_000), clientY: y });
    await imagesIdle(page);
    assert.equal(repeatedImageRequests, 0, "reselecting a displayed screenshot must reuse its image");
    await page.evaluate(() => window.__selectedCanvases = [...document.querySelectorAll("#images canvas")]);
    await timeline.dispatchEvent("click", { clientX: await xAt(page, BASE + 60_000), clientY: y });
    await page.waitForFunction(first => document.querySelector("#selected").textContent !== first
      && document.querySelector("#images").getAttribute("aria-busy") === "false", selectedFirst);
    page.off("request", countImages);
    assert.equal(repeatedImageRequests, 0, "different capture IDs sharing an image must reuse its decoded content");
    assert.equal(await page.evaluate(() => [...document.querySelectorAll("#images canvas")].every((canvas, i) => canvas === window.__selectedCanvases[i])), true,
      "selecting another capture must reuse mounted monitor canvases");

    // Hover follows every pointer event and samples evidence at most every 100 ms.
    let movingSelections = 0;
    const countSelections = (request) => { if (request.url().endsWith("/api/query/selection")) movingSelections++; };
    page.on("request", countSelections);
    const selectedBeforeMovement = await page.locator("#images").innerHTML();
    const headingBeforeMovement = await page.locator("#selected").innerText();
    const box = await timeline.boundingBox();
    const startX = await xAt(page, BASE + 60_000);
    await page.evaluate(() => window.__timelineFormatters = 0);
    for (let i = 0; i < 6; i++) {
      const x = startX + 3 * i;
      await timeline.dispatchEvent("pointermove", { clientX: x, clientY: y, buttons: 0 });
      const position = await page.evaluate(() => ({
        line: parseFloat(document.querySelector("#timeline .hover-line").style.left),
        popup: parseFloat(document.querySelector("#timeline-tooltip").style.left),
      }));
      assert.ok(Math.abs(position.line - 100 * (x - box.x) / box.width) < 0.2, "hover line must follow each event before evidence settles");
      assert.ok(Math.abs(position.popup - Math.max(8, Math.min(1440 - 328, x - 160))) < 0.01, "popup must follow each event");
      await page.waitForTimeout(20);
    }
    assert.ok(movingSelections > 0, "continuous movement must sample hover evidence without waiting for a stop");
    assert.equal(await page.locator("#images").innerHTML(), selectedBeforeMovement, "hover never replaces the selected moment");
    assert.equal(await page.locator("#selected").innerText(), headingBeforeMovement);
    await page.waitForTimeout(150);
    page.off("request", countSelections);
    assert.ok(movingSelections <= 3, "hover sampling must coalesce rapid pointer events");
    const hoverFormatters = await page.evaluate(() => window.__timelineFormatters);
    assert.ok(hoverFormatters <= 2, "hover must reuse date formatters; constructed " + hoverFormatters);
    await page.waitForFunction(() => document.querySelectorAll("#timeline-tooltip canvas").length === 2);
    assert.match(await page.locator("#timeline-tooltip").innerText(), /Editor/);
    assert.match(await page.locator("#timeline-tooltip").innerText(), /<script>Planning notes<\/script>/);
    assert.equal(await page.locator("#timeline-tooltip script").count(), 0);
    const popup = await page.locator("#timeline-tooltip").boundingBox();
    assert.ok(popup.x >= 0 && popup.x + popup.width <= 1440 && popup.y >= 0 && popup.y + popup.height <= 1000);
    await timeline.dispatchEvent("pointermove", { clientX: box.x + box.width - 1, clientY: y, buttons: 0 });
    await page.waitForFunction(() => document.querySelector("#timeline-tooltip")?.textContent.includes("No screenshot"));
    assert.match(await page.locator("#timeline-tooltip").innerText(), /Not recording · paused/);
    const edgePopup = await page.locator("#timeline-tooltip").boundingBox();
    assert.ok(edgePopup.x >= 0 && edgePopup.x + edgePopup.width <= 1440);
    await timeline.press("Escape");
    assert.equal(await page.locator("#timeline-tooltip").count(), 0);
    await timeline.dispatchEvent("pointerleave");
    assert.equal(await page.locator("#timeline-tooltip").count(), 0);
    await imagesIdle(page);
    assert.match(await page.locator("#focus").innerText(), /<script>Planning notes<\/script>\s+Editor/);
    assert.equal(await page.locator("#focus script").count(), 0);

    // The breakdown follows the visible range; categories come from the fixture's settings.
    await page.getByRole("tab", { name: "Breakdown" }).click();
    assert.match(await page.locator("#application-breakdown").innerText(), /Editor\s+1h\s+100%/);
    assert.match(await page.locator("#title-breakdown").innerText(), /<script>Planning notes<\/script>/);
    assert.match(await page.locator("#category-breakdown li").first().innerText(), /Planning\s+1h\s+100%/);
    assert.match(await page.locator(".summary .scope").innerText(), /Visible range · 03:5\d–05:0\d/);
    // Color by follows categories by default and is remembered per browser.
    const segment = page.locator("#timeline .segment", { hasText: "Planning" }).first();
    assert.match(await segment.getAttribute("style"), /var\(--series-1\)/);
    assert.equal(await page.locator(".color-by button", { hasText: "Subcategory" }).count(), 1);
    await page.locator(".color-by button", { hasText: "App" }).click();
    assert.equal(await page.locator("#timeline .segment", { hasText: "Editor" }).count(), 1);
    assert.equal(await page.evaluate(() => localStorage.getItem("timeline.colorBy")), "application");
    await page.locator(".color-by button", { hasText: /^Category$/ }).click();
    assert.equal(await page.evaluate(() => localStorage.getItem("timeline.colorBy")), "category");

    // The longest stretch is marked, its start selected and the timeline focused.
    await page.locator("#day-totals button", { hasText: "Planning" }).click();
    await page.waitForFunction(() => document.querySelector("#stretch-highlight")
      && /04:00:00/.test(document.querySelector("#selected").textContent)
      && document.querySelector("#images").getAttribute("aria-busy") === "false");
    assert.equal(await page.evaluate(() => document.activeElement.id), "timeline");
    // Sessions do the same.
    await page.getByRole("tab", { name: "Sessions" }).click();
    assert.equal(await page.locator("#sessions .session.active").count(), 1, "the selected moment's session is highlighted");

    // One monitor at a time, then all again.
    await page.locator(".stage .segmented button", { hasText: "Display B" }).click();
    await page.waitForFunction(() => document.querySelectorAll("#images canvas").length === 1);
    assert.equal(await page.locator("#images button.shot").getAttribute("aria-label"), "Enlarge Display B");

    // The enlarged screenshot: fit, native size centered on the click, scrolling and closing.
    await page.locator("#images button.shot").click();
    assert.equal(await page.locator("#enlarged").evaluate((d) => d.open), true);
    const previewScroll = await page.evaluate(() => window.scrollY);
    assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).overflow), "hidden");
    await page.mouse.move(5, 5);
    await page.mouse.wheel(0, 400);
    await page.waitForTimeout(100);
    assert.equal(await page.evaluate(() => window.scrollY), previewScroll, "backdrop wheel must not scroll the page");
    await page.locator("#large-image canvas").evaluate(canvas => {
      canvas.width = 2400; canvas.height = 1600;
      const context = canvas.getContext("2d");
      context.fillStyle = "gray"; context.fillRect(0, 0, 2400, 1600);
    });
    const fitWidth = await page.locator("#large-image canvas").evaluate(img => img.getBoundingClientRect().width);
    assert.ok(fitWidth <= 1440 - 40);
    const fitImage = await page.locator("#large-image canvas").boundingBox();
    await page.mouse.click(fitImage.x + fitImage.width * 0.65, fitImage.y + fitImage.height * 0.65);
    const zoomPoint = await page.locator("#large-image").evaluate(viewport => {
      const image = viewport.querySelector("canvas").getBoundingClientRect();
      const bounds = viewport.getBoundingClientRect();
      return { x: image.left + image.width * 0.65 - bounds.left, y: image.top + image.height * 0.65 - bounds.top,
        centerX: viewport.clientWidth / 2, centerY: viewport.clientHeight / 2 };
    });
    assert.ok(Math.abs(zoomPoint.x - zoomPoint.centerX) <= 1, "zoom must center the clicked horizontal image position");
    assert.ok(Math.abs(zoomPoint.y - zoomPoint.centerY) <= 1, "zoom must center the clicked vertical image position");
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), true, "image clicks must not dismiss preview");
    assert.equal(await page.locator("#large-image canvas").evaluate(img => img.getBoundingClientRect().width), 2400);
    await page.locator("#large-image").hover();
    await page.mouse.wheel(0, 200);
    await page.waitForFunction(() => document.querySelector("#large-image").scrollTop > 0);
    assert.equal(await page.evaluate(() => window.scrollY), previewScroll, "native image scrolling must not move the page");
    await page.getByRole("button", { name: "Fit image to window" }).click();
    assert.equal(await page.locator("#large-image canvas").evaluate(img => img.getBoundingClientRect().width), fitWidth);
    await page.mouse.click(5, 5);
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), false);
    await page.locator("#images button.shot").click();
    assert.equal(await page.locator(".image-zoom").getAttribute("aria-pressed"), "false");
    await page.locator("#close-image").click();
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), false);
    await page.locator("#images button.shot").click();
    await page.keyboard.press("Escape");
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), false);
    assert.notEqual(await page.evaluate(() => getComputedStyle(document.documentElement).overflow), "hidden",
      "closing the preview must restore page scrolling");
    // Stepping inside the preview keeps the dialog and canvas mounted.
    await page.locator("#images button.shot").click();
    await page.evaluate(() => window.__previewCanvas = document.querySelector("#large-image canvas"));
    const headingBeforeStep = await page.locator("#selected").innerText();
    await page.keyboard.press("ArrowRight");
    await page.waitForFunction(before => document.querySelector("#selected").textContent !== before
      && document.querySelector("#images").getAttribute("aria-busy") === "false", headingBeforeStep);
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), true);
    assert.equal(await page.evaluate(() => window.__previewCanvas === document.querySelector("#large-image canvas")), true);
    await page.keyboard.press("Escape");

    // Zooming keeps the timeline and selected screenshot while fetching.
    let releaseZoom, zoomStarted, zoomFinished;
    const zoomPending = new Promise((resolve) => (zoomStarted = resolve));
    const zoomComplete = new Promise((resolve) => (zoomFinished = resolve));
    await page.route("**/api/query/overview", async (route) => {
      const response = await route.fetch();
      zoomStarted();
      await new Promise((resolve) => (releaseZoom = resolve));
      try { await route.fulfill({ response }); } finally { zoomFinished(); }
    });
    await page.locator("#zoom").selectOption("4");
    await zoomPending;
    const segmentsDuringZoom = await page.locator("#timeline .segment").count();
    const imagesDuringZoom = await page.locator("#images canvas").count();
    releaseZoom();
    await zoomComplete;
    await page.unroute("**/api/query/overview");
    assert.ok(segmentsDuringZoom > 0, "zoom must not blank the timeline while fetching");
    assert.equal(imagesDuringZoom, 1, "zoom must keep the selected screenshot visible");
    await timelineIdle(page);
    assert.equal(await page.locator(".scrubber .strip").count(), 1, "zoomed in, the whole day shows as an overview strip");

    await page.locator("#pause").click();
    await page.waitForFunction(() => document.querySelector("#recording").textContent === "Recording paused");
    await page.locator("#pause").click();
    await page.waitForFunction(() => document.querySelector("#recording").textContent === "Recording");

    // Delay date resolution: keep the complete previous view, and let a newer date win.
    let release, intercepted;
    const pending = new Promise((resolve) => (intercepted = resolve));
    await page.route("**/api/query/resolve_range", async (route) => {
      if (route.request().postDataJSON().date === "2026-09-21") {
        const response = await route.fetch();
        intercepted();
        await new Promise((resolve) => (release = resolve));
        try { await route.fulfill({ response }); } catch {}
      } else await route.continue();
    });
    // The previous view stays, apart from a mark tied to it.
    const tracks = () => timeline.evaluate(node => { const copy = node.cloneNode(true); copy.querySelector("#stretch-highlight")?.remove(); return copy.innerHTML; });
    const beforeResolve = await tracks();
    await chooseDate(page, "2026-09-21");
    await pending;
    assert.equal(await page.locator("#previous").isEnabled(), true, "a pending date must not block newer navigation");
    assert.equal(await page.locator("#next").isEnabled(), true);
    assert.equal(await tracks(), beforeResolve, "date resolution must retain the complete timeline and scale");
    await chooseDate(page, "2026-09-23");
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-23"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    release();
    await page.waitForTimeout(150);
    assert.equal(await page.locator("#date").inputValue(), "2026-09-23");
    assert.equal(await page.locator("#images canvas").count(), 1, "date navigation must preserve the selected image");
    assert.equal(await page.locator("#zoom").inputValue(), "1", "a day without activity opens whole");
    await page.unroute("**/api/query/resolve_range");
    await page.locator("#previous").click();
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-22"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#zoom").inputValue(), "fit", "a day with activity opens on it");
    await page.locator("#zoom").selectOption("24");
    await timelineIdle(page);
    await page.locator("#next").click();
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-23"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#zoom").inputValue(), "1", "another day resets zoom");
    await page.locator("#previous").click();
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-22"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await page.locator("#zoom").selectOption("1");
    await timelineIdle(page);

    // A delayed selection must not restore old focus or images after a later click.
    const fullBox = await timeline.boundingBox();
    let releaseSelection, selected, delayed = false;
    const selectedPending = new Promise((resolve) => (selected = resolve));
    await page.route("**/api/query/selection", async (route) => {
      if (!delayed) {
        delayed = true;
        const response = await route.fetch();
        selected();
        await new Promise((resolve) => (releaseSelection = resolve));
        try { await route.fulfill({ response }); } catch {}
      } else await route.continue();
    });
    const headingBeforePendingSelection = await page.locator("#selected").innerText();
    await page.mouse.click(fullBox.x + 1, y);
    await selectedPending;
    assert.equal(await page.locator("#selected").innerText(), headingBeforePendingSelection,
      "pending evidence must retain the timestamp of the displayed screenshot");
    await page.mouse.click(fullBox.x + fullBox.width * 0.9, y);
    await page.waitForFunction(() => document.querySelector("#images").textContent.includes("No screenshot"));
    releaseSelection();
    await page.waitForTimeout(150);
    assert.equal(await page.locator("#images canvas").count(), 0);
    await page.unroute("**/api/query/selection");
    let releaseImage, imageStarted, imageDelayed = false;
    const imagePending = new Promise((resolve) => (imageStarted = resolve));
    await page.route("**/api/tiles/*", async (route) => {
      if (!imageDelayed) {
        imageDelayed = true;
        const response = await route.fetch();
        imageStarted();
        await new Promise((resolve) => (releaseImage = resolve));
        try { await route.fulfill({ response }); } catch {}
      } else await route.continue();
    });
    await page.locator(".stage .segmented button", { hasText: "All" }).click();
    await page.mouse.click(fullBox.x + 1, y);
    await imagePending;
    await page.mouse.click(fullBox.x + fullBox.width * 0.9, y);
    await page.waitForFunction(() => document.querySelector("#images").textContent.includes("No screenshot"));
    releaseImage();
    await page.waitForTimeout(150);
    assert.equal(await page.locator("#images canvas").count(), 0);
    await page.unroute("**/api/tiles/*");
    const committed = await page.locator("#selected").innerText();

    // A pending hover keeps the selected moment and marks its preview as waiting.
    let releaseHover, hoverStarted, hoverFinished;
    const hoverPending = new Promise((resolve) => (hoverStarted = resolve));
    const hoverComplete = new Promise((resolve) => (hoverFinished = resolve));
    await page.route("**/api/query/selection", async (route) => {
      const response = await route.fetch();
      hoverStarted();
      await new Promise((resolve) => (releaseHover = resolve));
      try { await route.fulfill({ response }); } finally { hoverFinished(); }
    });
    const emptyBeforeHover = await page.locator("#images").innerHTML();
    const focusBeforeHover = await page.locator("#focus").innerHTML();
    const cursorBeforeHover = await page.locator("#cursor").evaluate(n => n.style.left);
    await page.mouse.move(fullBox.x + fullBox.width * 0.5, y);
    await hoverPending;
    assert.ok(Math.abs(await page.locator("#timeline .hover-line").evaluate(n => parseFloat(n.style.left)) - 50) < 0.1);
    assert.equal(await page.locator("#cursor").evaluate(n => n.style.left), cursorBeforeHover);
    assert.equal(await page.locator("#timeline-tooltip .preview.waiting").count(), 1);
    releaseHover();
    await hoverComplete;
    await page.unroute("**/api/query/selection");
    assert.equal(await page.locator("#images").innerHTML(), emptyBeforeHover, "hover must not touch the selected screenshot");
    assert.equal(await page.locator("#focus").innerHTML(), focusBeforeHover, "pending hover must not collapse the focus area");
    await page.waitForFunction(() => !document.querySelector("#timeline-tooltip .preview.waiting"));
    assert.equal(await page.locator("#selected").innerText(), committed, "hover must not change the selected heading");
    await page.mouse.move(5, 5);
    assert.equal(await page.locator("#timeline .hover-line").count(), 0);

    // Wheel zoom stays anchored to the pointer and synchronizes the zoom control.
    const wheelBox = await timeline.boundingBox();
    const wheel = { deltaY: -120, clientX: Math.floor(wheelBox.x + wheelBox.width / 4), clientY: y };
    const anchor = (wheel.clientX - wheelBox.x) / wheelBox.width;
    const zoomRequest = page.waitForRequest((request) => request.url().endsWith("/api/query/overview") && request.postDataJSON().range);
    await timeline.dispatchEvent("wheel", wheel);
    const zoomRange = (await zoomRequest).postDataJSON().range;
    assert.equal(await page.locator("#zoom").inputValue(), "2");
    assert.equal(zoomRange.end_ms - zoomRange.start_ms, 12 * 60 * 60 * 1000);
    assert.ok(Number.isInteger(zoomRange.start_ms));
    assert.ok(Math.abs(zoomRange.start_ms + (zoomRange.end_ms - zoomRange.start_ms) * anchor - (BASE + 24 * 60 * 60 * 1000 * anchor)) <= 1);
    await timelineIdle(page);
    await page.waitForTimeout(200);
    const unzoomRequest = page.waitForRequest((request) => request.url().endsWith("/api/query/overview") && request.postDataJSON().range);
    await timeline.dispatchEvent("wheel", { ...wheel, deltaY: 120 });
    const unzoomRange = (await unzoomRequest).postDataJSON().range;
    assert.equal(await page.locator("#zoom").inputValue(), "1");
    assert.equal(unzoomRange.start_ms, BASE);
    assert.equal(unzoomRange.end_ms - unzoomRange.start_ms, 24 * 60 * 60 * 1000);
    await timeline.dispatchEvent("pointerleave");
    await timelineIdle(page);
    await imagesIdle(page);

    // The live marker moves independently and disappears outside the visible day.
    await page.clock.setFixedTime(new Date(BASE + 12 * 60 * 60 * 1000));
    await page.waitForFunction(() => document.querySelector("#timeline .now")?.style.left === "50%");
    await page.clock.setFixedTime(new Date(BASE + 24 * 60 * 60 * 1000));
    await page.waitForFunction(() => !document.querySelector("#timeline .now"));

    // Live evidence appears without replacing the selected moment.
    // Within the two minutes appended from 05:00:05; synthetic events take fractional pixels.
    const live = BASE + 3_660_000;
    await timeline.dispatchEvent("click", { clientX: await xAt(page, live), clientY: y });
    await page.waitForFunction(() => document.querySelector("#images").textContent.includes("No screenshot")
      && document.querySelector("#images").getAttribute("aria-busy") === "false");
    const axisBeforeRefresh = await page.locator(".scrubber .axis").innerText();
    const totalsBeforeRefresh = await page.locator("#day-totals").innerText();
    const appended = new Promise(resolve => lines.once("line", resolve));
    fixture.stdin.write("append\n");
    assert.equal(await appended, "appended");
    await page.waitForFunction(() => document.querySelector('#timeline .shots[title="Screenshots · Display C"]'), null, { timeout: 7000 });
    await page.getByRole("tab", { name: "Breakdown" }).click();
    assert.match(await page.locator("#application-breakdown").innerText(), /Browser/);
    assert.match(await page.locator("#category-breakdown").innerText(), /Research/, "live updates refresh category totals");
    // The day's figures reread at most every 15 seconds.
    await page.waitForFunction(before => document.querySelector("#day-totals").innerText !== before, totalsBeforeRefresh, { timeout: 20000 });
    assert.match(await page.locator("#day-totals").innerText(), /Focus\s+1h 2m/, "live updates refresh the day's figures");
    await page.getByRole("tab", { name: "Sessions" }).click();
    await page.waitForFunction(() => document.querySelectorAll("#sessions > li").length === 2);
    assert.match(await page.locator("#sessions").innerText(), /Research[\s\S]*Added while open/, "the pause keeps the sessions apart");
    assert.equal(await page.locator("#images canvas").count(), 0, "live refresh must not replace the selected preview");
    await timeline.dispatchEvent("click", { clientX: await xAt(page, live), clientY: y });
    await page.waitForFunction(() => document.querySelectorAll("#images canvas").length === 1);
    assert.match(await page.locator("#focus").innerText(), /Added while open/);
    assert.match(await page.locator("#selected").innerText(), /^05:0[01]:/);
    assert.equal(await page.locator(".scrubber .axis").innerText(), axisBeforeRefresh);
    assert.equal(await page.locator("#zoom").inputValue(), "1");

    // A delayed background refresh must neither overlap itself nor restore an old day.
    let releaseRefresh, refreshStarted, refreshFinished, backgroundRequests = 0;
    const refreshPending = new Promise(resolve => refreshStarted = resolve);
    const refreshComplete = new Promise(resolve => refreshFinished = resolve);
    await page.route("**/api/query/overview_updates", async route => {
      if (route.request().postDataJSON().range && backgroundRequests++ === 0) {
        const response = await route.fetch();
        refreshStarted();
        await new Promise(resolve => releaseRefresh = resolve);
        try { await route.fulfill({ response }); } catch {}
        finally { refreshFinished(); }
      } else await route.continue();
    });
    const imagesBeforeRefresh = await page.locator("#images").innerHTML();
    await page.evaluate(() => { window.dispatchEvent(new Event("focus")); window.dispatchEvent(new Event("focus")); });
    await refreshPending;
    assert.equal(backgroundRequests, 1);
    assert.equal(await page.locator("#images").innerHTML(), imagesBeforeRefresh);
    await chooseDate(page, "2026-09-23");
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-23"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    releaseRefresh();
    await refreshComplete;
    await page.unroute("**/api/query/overview_updates");
    assert.equal(await page.locator("#date").inputValue(), "2026-09-23");
    assert.equal(await page.locator("#timeline .segment").count(), 0, "the old day's refresh must not reappear");
    await page.waitForFunction(() => !document.querySelector("#zoom").disabled
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");

    // A wheel gesture updates the scale immediately but fetches only its final range.
    let wheelReads = 0;
    const countWheelReads = request => { if (request.url().endsWith("/api/query/overview")) wheelReads++; };
    page.on("request", countWheelReads);
    const finalBox = await timeline.boundingBox();
    const wheelEvent = { clientX: finalBox.x + finalBox.width / 2, clientY: y, deltaY: -120 };
    await timeline.dispatchEvent("wheel", wheelEvent);
    assert.equal(await page.locator("#zoom").inputValue(), "2");
    await page.waitForTimeout(190);
    await timeline.dispatchEvent("wheel", wheelEvent);
    assert.equal(await page.locator("#zoom").inputValue(), "4");
    await page.waitForTimeout(190);
    await timeline.dispatchEvent("wheel", { ...wheelEvent, deltaY: 120 });
    await timelineIdle(page);
    page.off("request", countWheelReads);
    assert.equal(wheelReads, 1, "wheel movement should fetch only the final range");
    assert.equal(await page.locator("#zoom").inputValue(), "2");
    const minuteRequest = page.waitForRequest(request => request.url().endsWith("/api/query/overview"));
    await page.locator("#zoom").selectOption({ label: "1 minute" });
    const minuteRange = (await minuteRequest).postDataJSON().range;
    assert.equal(minuteRange.end_ms - minuteRange.start_ms, 60_000);
    await timelineIdle(page);
    await timeline.dispatchEvent("wheel", wheelEvent);
    assert.equal(await page.locator("#zoom").inputValue(), "1440", "wheel zoom stops at one minute");

    // Drag a one-minute range left: content follows the pointer, selection stays put.
    await page.mouse.move(5, 5);
    await imagesIdle(page);
    await timeline.scrollIntoViewIfNeeded();
    const dragBox = await timeline.boundingBox();
    const dragY = await bandY(page);
    const selectedBeforeDrag = await timeline.getAttribute("aria-valuetext");
    const axisBeforeDrag = await page.locator(".scrubber .axis").innerText();
    let dragReads = 0;
    const countDragReads = request => { if (request.url().endsWith("/api/query/overview")) dragReads++; };
    page.on("request", countDragReads);
    await page.mouse.move(dragBox.x + dragBox.width * 0.6, dragY);
    await page.mouse.down();
    await page.mouse.move(dragBox.x + dragBox.width * 0.35, dragY, { steps: 5 });
    await page.waitForTimeout(50);
    assert.notEqual(await page.locator(".scrubber .axis").innerText(), axisBeforeDrag, "drag must immediately reposition the range");
    assert.equal(await timeline.getAttribute("aria-valuetext"), selectedBeforeDrag, "drag must not select another time");
    await page.waitForTimeout(250);
    assert.equal(dragReads, 0, "hold the existing tracks until drag release");
    const dragRequest = page.waitForRequest(request => request.url().endsWith("/api/query/overview"));
    await page.mouse.up();
    const draggedRange = (await dragRequest).postDataJSON().range;
    assert.ok(Math.abs(draggedRange.start_ms - minuteRange.start_ms - 15_000) < 100,
      "drag distance must map proportionally to the visible time range");
    await timelineIdle(page);
    page.off("request", countDragReads);
    assert.equal(dragReads, 1);
    assert.equal(await timeline.getAttribute("aria-valuetext"), selectedBeforeDrag);
    await page.mouse.move(5, 5);

    // Navigation controls select real captures, fit recorded activity and cross days.
    await chooseDate(page, "2026-09-22");
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-22"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await page.locator("#zoom").selectOption("1");
    await timelineIdle(page);
    const navBox = await timeline.boundingBox();
    await timeline.dispatchEvent("click", { clientX: navBox.x, clientY: y });
    await imagesIdle(page);
    const nextCapture = page.waitForRequest(request => request.url().endsWith("/api/query/selection") && request.postDataJSON().selected_ms === BASE + 5000);
    await timeline.press("ArrowRight");
    await nextCapture;
    await imagesIdle(page);
    // Arrow keys work from anywhere on the page, too.
    const previousCapture = page.waitForRequest(request => request.url().endsWith("/api/query/selection") && request.postDataJSON().selected_ms === BASE);
    await page.evaluate(() => document.activeElement.blur());
    await page.keyboard.press("ArrowLeft");
    await previousCapture;
    await imagesIdle(page);
    const beforeFitImage = await page.locator("#images").innerHTML();
    const fitQuery = page.waitForRequest(request => request.url().endsWith("/api/query/overview") && request.postDataJSON().range);
    await page.locator(".scrubber button", { hasText: "Fit" }).click();
    const fitted = (await fitQuery).postDataJSON().range;
    assert.ok(fitted.start_ms <= BASE && fitted.end_ms >= BASE + 3725000);
    assert.ok(fitted.end_ms - fitted.start_ms < 2 * 3600000);
    await timelineIdle(page);
    assert.equal(await page.locator("#zoom").inputValue(), "fit");
    assert.equal(await page.locator("#images").innerHTML(), beforeFitImage);
    await page.clock.setFixedTime(new Date(BASE + 86400000 + 5000));
    await page.locator("#now").click();
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-23" && document.querySelector("#images").getAttribute("aria-busy") === "false" && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await timeline.press("ArrowLeft");
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-22" && document.querySelector("#images").getAttribute("aria-busy") === "false" && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.match(await page.locator("#focus").innerText(), /Added while open/);
    await page.locator("#zoom").selectOption("1");
    await timelineIdle(page);
    await page.getByRole("tab", { name: "Breakdown" }).click();
    await timeline.scrollIntoViewIfNeeded();
    const crossDayBox = await timeline.boundingBox();
    const crossDayY = await bandY(page);
    const beforeCrossDayImage = await page.locator("#images").innerHTML();
    const beforeCrossDaySegments = await timeline.locator(".segment").count();
    const beforeCrossDayRanking = await page.locator("#application-breakdown").innerHTML();
    const beforeCrossDayScope = await page.locator(".summary .scope").innerText();
    let releaseCrossDay, crossDayStarted, crossDayDelayed = false;
    const crossDayPending = new Promise(resolve => crossDayStarted = resolve);
    const crossDayQueries = [];
    await page.route("**/api/query/overview", async route => {
      crossDayQueries.push(route.request().postDataJSON());
      if (!crossDayDelayed) {
        crossDayDelayed = true;
        const response = await route.fetch();
        crossDayStarted();
        await new Promise(resolve => releaseCrossDay = resolve);
        await route.fulfill({ response });
      } else await route.continue();
    });
    await page.mouse.move(crossDayBox.x + crossDayBox.width * 0.8, crossDayY);
    await page.mouse.down();
    await page.mouse.move(crossDayBox.x + crossDayBox.width * 0.1, crossDayY, { steps: 5 });
    assert.equal(await page.locator("#application-breakdown").innerHTML(), beforeCrossDayRanking, "dragging must retain the loaded breakdown");
    await page.mouse.up();
    await crossDayPending;
    assert.equal(await timeline.locator(".segment").count(), beforeCrossDaySegments,
      "a full-day drag must keep previously loaded events while loading");
    assert.equal((await timeline.boundingBox()).height, crossDayBox.height, "the timeline must not collapse during date navigation");
    assert.equal(await page.locator("#application-breakdown").innerHTML(), beforeCrossDayRanking);
    assert.equal(await page.locator(".summary .scope").innerText(), beforeCrossDayScope,
      "retained totals must remain labeled with their actual source range");
    releaseCrossDay();
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-23" && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#images").innerHTML(), beforeCrossDayImage);
    await page.unroute("**/api/query/overview");
    assert.ok(crossDayQueries.length > 0);
    assert.ok(crossDayQueries.every(args => args.range && !args.date),
      "cross-day navigation fetches the dragged range without a discarded date overview");
    assert.ok(crossDayQueries.slice(1).every(args => args.cursor), "one overview snapshot, with only pagination requests after it");
    // An unchanged focus refresh is a small delta and never a full overview reload.
    let fullReads = 0;
    const countFullReads = request => { if (request.url().endsWith("/api/query/overview")) fullReads++; };
    page.on("request", countFullReads);
    const deltaResponse = page.waitForResponse(response => response.url().endsWith("/api/query/overview_updates"));
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    const delta = await (await deltaResponse).json();
    assert.equal(delta.reset, false);
    assert.deepEqual(delta.changes.items, []);
    assert.equal(fullReads, 0);
    page.off("request", countFullReads);

    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(300);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), "the day view must fit a narrow screen");
    await timeline.scrollIntoViewIfNeeded();
    const mobileBox = await timeline.boundingBox();
    const stageBeforeHover = await page.locator(".stage").boundingBox();
    const panelBeforeHover = await page.locator(".panel").boundingBox();
    await timeline.dispatchEvent("pointermove", { clientX: mobileBox.x + 5, clientY: mobileBox.y + 30, buttons: 0 });
    assert.deepEqual(await page.locator(".stage").boundingBox(), stageBeforeHover, "hovering must not move the page");
    assert.deepEqual(await page.locator(".panel").boundingBox(), panelBeforeHover);
    await timeline.dispatchEvent("pointerleave");
    await page.setViewportSize({ width: 1440, height: 1000 });
    await timelineIdle(page);

    assert.deepEqual(errors, []);
    await page.close();
    // Browser closure leaves the Rust service running.
    assert.equal(fixture.exitCode, null);
  } finally {
    await browser.close();
    await stopFixture(fixture);
  }
});

test("commit markers cluster, explain themselves, select their moment and report unreadable repositories", async () => {
  const { fixture, url } = await startFixture();
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.clock.setFixedTime(new Date("2026-09-23T12:00:00Z"));
    const launch = new URL(url);
    await page.goto(`${launch.origin}/?date=2026-09-22${launch.hash}`);
    await page.waitForFunction(() => document.querySelectorAll("#commit-track .commit").length
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    // Commits at 04:10:00 and 04:10:30 share a marker on the opened hour; 04:35 and 04:55 stand apart.
    const markers = page.locator("#commit-track .commit");
    assert.equal(await markers.count(), 3);
    assert.equal(await markers.first().innerText(), "2");
    assert.match(await page.locator("#day-totals").innerText(), /Commits\s+4/);
    const y = (await page.locator("#commit-track").boundingBox()).y + 8;
    await page.mouse.move(await xAt(page, BASE + 615_000), y);
    const tooltip = page.locator("#timeline-tooltip");
    await page.waitForFunction(() => document.querySelector("#timeline-tooltip .commit-list"));
    assert.match(await tooltip.innerText(), /Plan the commit track\s+timeline · 04:10:00[\s\S]*Fix marker spacing/);
    // Near a commit's time but off its marker, the timeline selects where it was clicked.
    const band = await bandY(page);
    await page.mouse.move(await xAt(page, BASE + 2_100_000) + 3, band);
    assert.doesNotMatch(await tooltip.innerText(), /Notes on markers/, "commits show only over their marker");
    await page.mouse.click(await xAt(page, BASE + 2_100_000) + 3, band);
    await page.waitForFunction(() => /^04:35:\d\d$/.test(document.querySelector("#selected")?.textContent ?? ""));
    assert.notEqual(await page.locator("#selected").innerText(), "04:35:00");
    // Clicking a marker selects its first commit's moment.
    await page.mouse.click(await xAt(page, BASE + 2_100_000), y);
    await page.waitForFunction(() => document.querySelector("#selected")?.textContent.includes("04:35:00"));

    // Zooming in on the cluster separates its commits.
    await page.mouse.move(await xAt(page, BASE + 615_000), y);
    await page.mouse.wheel(0, -100);
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false"
      && document.querySelectorAll("#commit-track .commit").length >= 2 && !document.querySelector("#commit-track .many"));

    // The commits tab lists the loaded range's commits; each selects its moment.
    await page.getByRole("tab", { name: /Commits/ }).click();
    assert.match(await page.locator("#commit-list").innerText(), /04:10\s+Plan the commit track\s+timeline · main · [0-9a-f]{7}/);
    await page.locator("#commit-list button", { hasText: "Plan the commit track" }).click();
    await page.waitForFunction(() => document.querySelector("#selected")?.textContent.includes("04:10:00"));
    assert.equal(await page.evaluate(() => document.activeElement.id), "timeline");

    // Unreadable repositories open the repository details.
    assert.match(await page.locator(".commits .problem").innerText(), /1 repository or path could not be read/);
    await page.locator(".commits .problem").click();
    await page.waitForFunction(() => document.querySelector(".drawer").open && document.querySelector("#repositories-found"));
    assert.match(await page.locator(".drawer").innerText(), /2 repositories in[\s\S]*broken · broken \.git file/);
    // Repositories show as a folder tree, nested under their shared folder.
    assert.deepEqual(await page.locator("#repository-tree > li").evaluateAll(items => items.map(item => item.innerText.replace(/\s+/g, " ").trim())),
      ["projects 2 notes timeline"]);
    await page.locator("#repository-tree summary").click();
    assert.equal(await page.locator("#repository-tree details").evaluate(details => details.open), false);
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
    await stopFixture(fixture);
  }
});

test("week and month views summarize days, compare honestly and drill into the day view", async () => {
  const { fixture, url } = await startFixture();
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.clock.setFixedTime(new Date("2026-09-23T12:00:00Z"));
    // A linked week survives the credential exchange, normalized to its Monday.
    const launch = new URL(url);
    await page.goto(`${launch.origin}/?view=week&date=2026-09-23${launch.hash}`);
    await page.waitForFunction(() => document.querySelector("#period-totals .kpi"));
    assert.equal(new URL(page.url()).search, "?view=week&date=2026-09-21");
    assert.equal(await page.locator(".views [aria-current=page]").innerText(), "Week");
    assert.equal(await page.locator("#period-heading").innerText(), "Week 39");
    assert.match(await page.locator(".period .subtitle").innerText(), /21–27 Sept 2026/);
    assert.match(await page.title(), /Week of 21–27 Sept 2026/);
    assert.equal(await page.locator("#images").count(), 0, "the day view is not loaded behind a week");

    // One hour of Planning on Tuesday; Monday has no evidence; Thursday onwards has not begun.
    await page.waitForFunction(() => !document.querySelector("#period-focus").textContent.includes("…"));
    assert.match(await page.locator("#period-focus").innerText(), /1h\s*no comparison/);
    assert.match(await page.locator("#period-evidence").innerText(), /No recording on Mon 21 Sept, Wed 23 Sept/);
    const states = await page.locator(".week .column").evaluateAll(columns => columns.map(column =>
      ["missing", "future", "today"].filter(name => column.classList.contains(name)).join(" ") || "recorded"));
    assert.deepEqual(states, ["missing", "recorded", "missing today", "future", "future", "future", "future"]);
    await page.waitForFunction(() => document.querySelectorAll(".week .session").length === 1);
    const session = page.locator(".week .session");
    assert.match(await session.getAttribute("aria-label"), /^Planning, 04:00–05:00, 1h$/);
    assert.match(await session.getAttribute("style"), /var\(--series-1\)/, "categories keep their palette slot in every view");
    await session.hover();
    assert.match(await page.locator(".tooltip").innerText(), /Planning\s+04:00–05:00 · 1h\s+Editor · <script>Planning notes<\/script>/);
    assert.match(await page.locator("#subject-changes").innerText(), /Planning\s+1h/);
    // Commits per day: in the column headers and placed in the columns.
    await page.waitForFunction(() => document.querySelector("#period-commits"));
    assert.deepEqual(await page.locator(".week .column-head").evaluateAll(heads => heads.map(head => head.querySelector(".commit-count")?.textContent ?? "")),
      ["", "4", "1", "", "", "", ""], "a day without focus evidence can still have commits");
    assert.equal(await page.locator(".week .commit").count(), 5);
    assert.match(await page.locator("#period-commits").innerText(), /Commits\s+5/);
    assert.match(await page.locator("#period-evidence").innerText(), /1 repository or path could not be read/);
    await page.locator(".color-by button", { hasText: "App" }).click();
    assert.match(await page.locator("#subject-changes").innerText(), /Editor\s+1h/);
    await page.locator(".color-by button", { hasText: /^Category$/ }).click();

    // The previous week, then back to this one.
    await page.locator("#period-previous").click();
    await page.waitForFunction(() => location.search === "?view=week&date=2026-09-14" && document.querySelector(".period").getAttribute("aria-busy") === "false");
    await page.locator("#period-current").click();
    await page.waitForFunction(() => location.search === "?view=week&date=2026-09-21" && document.querySelector(".period").getAttribute("aria-busy") === "false");

    // A session opens its day with the session marked; Back returns to the week.
    await page.waitForFunction(() => document.querySelectorAll(".week .session").length === 1);
    await page.locator(".week .session").click();
    await page.waitForFunction(() => document.querySelector("#stretch-highlight") && document.querySelector("#date")?.value === "2026-09-22");
    assert.equal(new URL(page.url()).search, "?date=2026-09-22");
    assert.equal(await page.evaluate(() => document.activeElement.id), "timeline");
    await page.goBack();
    await page.waitForFunction(() => location.search === "?view=week&date=2026-09-21" && document.querySelector("#period-totals"));
    await page.locator(".week .column-head").nth(1).click();
    await page.waitForFunction(() => document.querySelector("#date")?.value === "2026-09-22"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await page.locator("#next").click();
    await page.waitForFunction(() => location.search === "?date=2026-09-23", "day navigation keeps the URL current");
    await page.reload();
    await page.waitForFunction(() => document.querySelector("#date")?.value === "2026-09-23");

    await page.locator(".views a", { hasText: "Month" }).click();
    await page.waitForFunction(() => location.search === "?view=month&date=2026-09-01" && document.querySelector("#period-totals .kpi"));
    assert.equal(await page.locator("#period-heading").innerText(), "September 2026");
    assert.equal(await page.locator(".month .cell").count(), 30);
    assert.match(await page.locator(".month .cell").nth(21).getAttribute("aria-label"), /^Tuesday 22 September: 1h focus$/);
    assert.equal(await page.locator(".month .cell.missing").count(), 22, "days that began without evidence");
    assert.ok(await page.locator(".rhythm .cell[style*='--seq']").count() >= 1, "Tuesday's active hour");
    await page.locator(".month .cell").nth(21).click();
    await page.waitForFunction(() => document.querySelector("#date")?.value === "2026-09-22");
    await page.goBack();
    await page.waitForFunction(() => document.querySelector(".month .cell"));
    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(200);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), "the month fits a narrow screen");
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
    await stopFixture(fixture);
  }
});
