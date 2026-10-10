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

test("bundled browser with real history, calendar, lanes, breakdown, monitors and obsolete responses", async () => {
  const { fixture, lines, url } = await startFixture();
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext({
      viewport: { width: 1440, height: 1000 },
    });
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

    let storageRequests = 0;
    page.on("request", request => { if (request.url().endsWith("/api/storage")) storageRequests++; });
    assert.equal(await page.locator(".storage-details").getAttribute("open"), null);
    assert.equal(storageRequests, 0);
    await page.locator(".storage-details summary").click();
    await page.waitForFunction(() => document.querySelector(".storage-body").getAttribute("aria-busy") === "false"
      && !document.querySelector(".storage-body dd").textContent.includes("—"));
    assert.equal(storageRequests, 1);
    assert.equal(await page.locator(".storage-body dd").count(), 6);
    await page.locator(".storage-details summary").click();
    await page.locator(".storage-details summary").click();
    assert.equal(storageRequests, 1, "opening a recent storage snapshot should not fetch again");
    await page.locator(".storage-details summary").click();
    await page.evaluate(() => window.scrollTo(0, 0));

    const emptyPage = await page.context().newPage();
    await emptyPage.clock.setFixedTime(new Date("2026-09-23T12:00:00Z"));
    await emptyPage.route("**/api/query/adjacent_screenshot", route => route.fulfill({json: null}));
    await emptyPage.goto(new URL("/", url).href);
    await emptyPage.waitForFunction(() => document.querySelector("#date").value === "2026-09-23"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.equal(await emptyPage.locator("#images canvas").count(), 0, "no captures leaves an unselected viewer on today's activity day");
    await emptyPage.close();
    await page.locator("#date").fill("2026-09-22");
    await page.locator("#date").dispatchEvent("change");
    await page.waitForFunction(() =>
      document.querySelector("#timeline .block") && document.querySelector("#timeline").getAttribute("aria-busy") === "false",
    );
    const coverageBands = await page.evaluate(() => {
      const label = [...document.querySelectorAll(".track-label")].find(n => n.textContent === "Screenshots · Display A");
      return [...label.nextElementSibling.querySelectorAll(".block")].map(n => n.dataset.description);
    });
    assert.equal(coverageBands[0], "Screenshot coverage");
    assert.ok(coverageBands.some(label => label === "fixture_gap" || label.includes("Mixed screenshot coverage")),
      "aggregation must retain even subpixel screenshot gaps");
    assert.equal(coverageBands.at(-1), "Screenshot coverage");
    const timeline = page.locator("#timeline");
    const box = await timeline.boundingBox();
    await page.mouse.click(box.x + 1, box.y + 15);
    await page.waitForFunction(
      () => document.querySelectorAll("#images canvas").length === 2 && document.querySelector("#images").getAttribute("aria-busy") !== "true",
    );
    let repeatedImageRequests = 0;
    const countImages = (request) => {
      if (request.url().includes("/api/tiles/")) repeatedImageRequests++;
    };
    page.on("request", countImages);
    await timeline.dispatchEvent("click", { clientX: box.x + 1, buttons: 1 });
    await page.waitForFunction(() => document.querySelector("#images").getAttribute("aria-busy") === "false");
    page.off("request", countImages);
    assert.equal(repeatedImageRequests, 0, "reselecting a displayed screenshot must reuse its image");
    repeatedImageRequests = 0;
    page.on("request", countImages);
    await page.evaluate(() => window.__selectedCanvases = [...document.querySelectorAll("#images canvas")]);
    await timeline.dispatchEvent("click", { clientX: box.x + 2, buttons: 1 });
    await page.waitForFunction(() => document.querySelector("#images").getAttribute("aria-busy") === "false");
    page.off("request", countImages);
    assert.equal(repeatedImageRequests, 0, "different capture IDs sharing an image must reuse its decoded content");
    assert.equal(await page.evaluate(() => [...document.querySelectorAll("#images canvas")].every((canvas, i) => canvas === window.__selectedCanvases[i])), true,
      "selecting another capture must reuse mounted monitor canvases");
    let movingSelections = 0;
    const countSelections = (request) => {
      if (request.url().endsWith("/api/query/selection")) movingSelections++;
    };
    page.on("request", countSelections);
    const selectedBeforeMovement = await page.locator("#images").innerHTML();
    const headingBeforeMovement = await page.locator("#selected").innerText();
    await page.evaluate(() => window.__timelineFormatters = 0);
    for (let i = 0; i < 6; i++) {
      const x = box.x + 1 + i;
      await timeline.dispatchEvent("pointermove", { clientX: x, clientY: box.y + 15, buttons: 0 });
      const position = await page.evaluate(() => ({
        cursor: parseFloat(document.querySelector("#hover-cursor").style.left),
        popup: parseFloat(document.querySelector("#timeline-tooltip").style.left),
      }));
      assert.ok(Math.abs(position.cursor - 100 * (1 + i) / box.width) < 0.001,
        "hover line must follow each event before evidence debounce settles");
      assert.ok(Math.abs(position.popup - (x + 16)) < 0.01,
        "popup must follow each event before evidence debounce settles");
      await page.waitForTimeout(20);
    }
    assert.ok(movingSelections > 0, "continuous movement must sample hover evidence without waiting for a stop");
    assert.equal(await page.locator("#images").innerHTML(), selectedBeforeMovement);
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
    await page.screenshot({ path: "/tmp/timeline-hover-popup.png", fullPage: true });
    await timeline.dispatchEvent("pointermove", { clientX: box.x + box.width - 1, clientY: box.y + box.height - 1, buttons: 0 });
    await page.waitForFunction(() => document.querySelector("#timeline-tooltip")?.textContent.includes("No screenshot evidence"));
    const edgePopup = await page.locator("#timeline-tooltip").boundingBox();
    assert.ok(edgePopup.x >= 0 && edgePopup.x + edgePopup.width <= 1440);
    await timeline.press("Escape");
    assert.equal(await page.locator("#timeline-tooltip").count(), 0);
    await timeline.dispatchEvent("pointerleave");
    assert.equal(await page.locator("#timeline-tooltip").count(), 0);
    await page.waitForFunction(() => document.querySelector("#images").getAttribute("aria-busy") === "false");
    assert.match(
      await page.locator("#focus").innerText(),
      /<script>Planning notes<\/script>/,
    );
    assert.equal(await page.locator("#focus script").count(), 0);
    await page.locator("#lanes").click();
    assert.equal(
      await page.locator("#lanes").getAttribute("aria-expanded"),
      "true",
    );
    assert.equal(await page.locator("#timeline").locator("..").locator("#breakdown").count(), 0);
    await page.locator("#breakdown summary", { hasText: "Window titles" }).click();
    assert.match(await page.locator("#titles").innerText(), /Editor/);
    assert.match(await page.locator("#applications").innerText(), /Editor/);
    // Categories come from the fixture's settings file: all recorded focus is Planning.
    assert.match(await page.locator("#categories li").first().innerText(), /Planning[\s\S]*100\.0%/);
    assert.match(await page.locator("#categories").innerText(), /Research[\s\S]*Communication/);
    assert.equal(await page.locator("#categories li.nested").innerText().then(text => text.split("\n")[0]), "Mail",
      "subcategories follow their parent, indented");
    assert.equal(await page.locator("#color-by label", { hasText: "Subcategory" }).count(), 1);
    assert.match(await page.locator("#categories-heading").innerText(), /100% categorized/);
    // Focus metrics: one uninterrupted hour on Planning, with no switches.
    assert.match(await page.locator("#switch-rate").innerText(), /0 per active hour[\s\S]*0 application switches · 0\/h between categories/);
    assert.match(await page.locator("#longest-stretch").innerText(), /1h\s+Planning · 04:00–05:00/);
    assert.match(await page.locator("#long-share").innerText(), /100%[\s\S]*1 stretch of 25 min or more/);
    assert.match(await page.locator("#stretch-distribution").innerText(), /25 min or more\s+1 stretch\s+1h\s+100%/);
    assert.equal(await page.locator("#switch-strip").count(), 1);
    assert.equal(await page.locator("#switch-strip .switch-cell").count(), 0, "hours without switches stay empty");
    await page.locator("#breakdown summary", { hasText: "Switches by hour" }).click();
    assert.match(await page.locator("#hourly-switches tbody").innerText(), /04:00–05:00\s+1h\s+0\s+0/);
    assert.match(await page.locator("#titles").innerText(), /Editor · Planning/);
    await page.locator("#color-by label", { hasText: /^Category$/ }).click();
    assert.match(await page.locator("#lanes").innerText(), /Category lanes/);
    assert.equal(await page.locator("#category-legend").innerText(), "Planning");
    const categoryBlock = page.locator("#timeline .block").first();
    assert.match(await categoryBlock.getAttribute("style"), /var\(--series-1\)/);
    assert.match(await categoryBlock.innerText(), /Planning/);
    assert.equal(await page.evaluate(() => localStorage.getItem("timeline.colorBy")), "category");
    await page.locator("#color-by label", { hasText: "Application" }).click();
    assert.match(await page.locator("#timeline .block").first().innerText(), /Editor/);
    await page.locator(".breakdown-details", { hasText: "Category rules" }).locator("summary").click();
    await page.waitForFunction(() => document.querySelectorAll("#category-rules > li").length === 4);
    assert.match(await page.locator("#category-rules").innerText(), /Research[\s\S]*Browser/);
    await page.locator("#longest-stretch button").click();
    await page.waitForFunction(() => document.querySelector("#stretch-highlight")
      && document.querySelector("#images").getAttribute("aria-busy") === "false");
    assert.match(await page.locator("#selected").innerText(), /04:00:00/, "revealing a stretch selects its start");
    assert.equal(await page.evaluate(() => document.activeElement.id), "timeline");
    await page.locator("#layout").selectOption("stack");
    await page.waitForFunction(
      () => document.querySelectorAll("#images canvas").length === 2,
    );
    assert.equal(await page.locator("#images").getAttribute("class"), "stack");
    await page.locator("#layout").selectOption("monitor:Display B");
    await page.waitForFunction(
      () => document.querySelectorAll("#images canvas").length === 1,
    );
    assert.match(await page.locator("#images").innerText(), /Display B/);
    await page.locator("#images button").click();
    assert.equal(await page.locator("#enlarged").evaluate((d) => d.open), true);
    assert.equal(await page.locator("#close-image").innerText(), "×");
    const previewScroll = await page.evaluate(() => window.scrollY);
    assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).overflow), "hidden");
    await page.mouse.move(5, 5);
    await page.mouse.wheel(0, 400);
    await page.waitForTimeout(100);
    assert.equal(await page.evaluate(() => window.scrollY), previewScroll, "backdrop wheel must not scroll the page");
    const pinnedImage = await page.locator("#images").innerHTML();
    await timeline.dispatchEvent("pointermove", { clientX: box.x + box.width * 0.9, clientY: box.y + 15, buttons: 0 });
    await page.waitForFunction(() => document.querySelector(".tooltip-preview")?.getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), true, "hover must not dismiss an enlarged selected image");
    assert.equal(await page.locator("#images").innerHTML(), pinnedImage);
    await timeline.dispatchEvent("pointerleave");
    // Use a large decoded image to verify actual fit/native geometry and overflow.
    await page.locator("#large-image canvas").evaluate(canvas => {
      canvas.width = 2400; canvas.height = 1600;
      const context = canvas.getContext("2d");
      context.fillStyle = "gray"; context.fillRect(0, 0, 2400, 1600);
    });
    const fitWidth = await page.locator("#large-image canvas").evaluate(img => img.getBoundingClientRect().width);
    assert.ok(fitWidth <= 1440 * 0.9);
    const fitImage = await page.locator("#large-image canvas").boundingBox();
    await page.mouse.click(fitImage.x + fitImage.width * 0.65, fitImage.y + fitImage.height * 0.65);
    const zoomPoint = await page.locator("#large-image").evaluate(viewport => {
      const image = viewport.querySelector("canvas").getBoundingClientRect();
      const bounds = viewport.getBoundingClientRect();
      return {
        x: image.left + image.width * 0.65 - bounds.left,
        y: image.top + image.height * 0.65 - bounds.top,
        centerX: viewport.clientWidth / 2,
        centerY: viewport.clientHeight / 2,
      };
    });
    assert.ok(Math.abs(zoomPoint.x - zoomPoint.centerX) <= 1, "zoom must center the clicked horizontal image position");
    assert.ok(Math.abs(zoomPoint.y - zoomPoint.centerY) <= 1, "zoom must center the clicked vertical image position");
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), true, "image clicks must not dismiss preview");
    assert.equal(await page.locator("#large-image canvas").evaluate(img => img.getBoundingClientRect().width), 2400);
    assert.ok(await page.locator("#large-image").evaluate(el => el.scrollWidth > el.clientWidth && el.scrollHeight > el.clientHeight));
    await page.locator("#large-image").hover();
    await page.mouse.wheel(0, 200);
    await page.waitForFunction(() => document.querySelector("#large-image").scrollTop > 0);
    assert.equal(await page.evaluate(() => window.scrollY), previewScroll, "native image scrolling must not move the page");
    await page.getByRole("button", { name: "Fit image to window" }).click();
    assert.equal(await page.locator("#large-image canvas").evaluate(img => img.getBoundingClientRect().width), fitWidth);
    await page.mouse.click(5, 5);
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), false);
    await page.locator("#images button").click();
    assert.equal(await page.locator(".image-zoom").getAttribute("aria-pressed"), "false");
    await page.locator("#close-image").click();
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), false);
    await page.locator("#images button").click();
    await page.keyboard.press("Escape");
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), false);
    assert.notEqual(await page.evaluate(() => getComputedStyle(document.documentElement).overflow), "hidden",
      "closing the preview must restore page scrolling");
    // Screenshot stepping inside the preview keeps the dialog and canvas mounted.
    await page.locator("#images button").click();
    await page.evaluate(() => window.__previewCanvas = document.querySelector("#large-image canvas"));
    const headingBeforeStep = await page.locator("#selected").innerText();
    await page.keyboard.press("ArrowRight");
    await page.waitForFunction(before => document.querySelector("#selected").textContent !== before
      && document.querySelector("#images").getAttribute("aria-busy") === "false", headingBeforeStep);
    assert.equal(await page.locator("#enlarged").evaluate(d => d.open), true);
    assert.equal(await page.evaluate(() => window.__previewCanvas === document.querySelector("#large-image canvas")), true);
    await page.keyboard.press("Escape");
    let releaseZoom;
    let zoomStarted;
    let zoomFinished;
    const zoomPending = new Promise((resolve) => (zoomStarted = resolve));
    const zoomComplete = new Promise((resolve) => (zoomFinished = resolve));
    await page.route("**/api/query/overview", async (route) => {
      const response = await route.fetch();
      zoomStarted();
      await new Promise((resolve) => (releaseZoom = resolve));
      try { await route.fulfill({ response }); }
      finally { zoomFinished(); }
    });
    await page.locator("#zoom").selectOption("4");
    await zoomPending;
    const tracksDuringZoom = await page.locator("#timeline .track").count();
    const imagesDuringZoom = await page.locator("#images canvas").count();
    releaseZoom();
    await zoomComplete;
    await page.unroute("**/api/query/overview");
    assert.ok(tracksDuringZoom > 0, "zoom must not blank the timeline while fetching");
    assert.equal(imagesDuringZoom, 1, "zoom must keep the selected screenshot visible");
    await page.waitForFunction(() =>
      document.querySelector("#timeline .block") && document.querySelector("#timeline").getAttribute("aria-busy") === "false",
    );
    assert.equal(
      await page.locator("#lanes").getAttribute("aria-expanded"),
      "true",
    );
    await page.locator("#pause").click();
    await page.waitForFunction(
      () =>
        document.querySelector("#recording").textContent === "Recording paused",
    );
    await page.locator("#pause").click();
    await page.waitForFunction(
      () => document.querySelector("#recording").textContent === "Recording",
    );
    // Delay date resolution: keep the complete previous view, and let a newer date win.
    let release;
    let intercepted;
    const pending = new Promise((resolve) => (intercepted = resolve));
    await page.route("**/api/query/resolve_range", async (route) => {
      const args = route.request().postDataJSON();
      if (args.date === "2026-09-21") {
        const response = await route.fetch();
        intercepted();
        await new Promise((resolve) => (release = resolve));
        try {
          await route.fulfill({ response });
        } catch {}
      } else await route.continue();
    });
    const beforeResolve = await timeline.innerHTML();
    const emptyBreakdownBeforeResolve = await page.locator("#breakdown .breakdown-empty").count();
    const breakdownBeforeResolve = await page.locator("#applications").count()
      ? await page.locator("#applications").innerHTML() : null;
    await page.locator("#date").fill("2026-09-21");
    await page.locator("#date").dispatchEvent("change");
    await pending;
    assert.equal(await page.locator("#previous").isEnabled(), true, "a pending date must not block newer navigation");
    assert.equal(await page.locator("#next").isEnabled(), true);
    assert.equal(await timeline.innerHTML(), beforeResolve, "date resolution must retain the complete timeline and scale");
    if (breakdownBeforeResolve !== null)
      assert.equal(await page.locator("#applications").innerHTML(), breakdownBeforeResolve);
    assert.equal(await page.locator("#breakdown .breakdown-empty").count(), emptyBreakdownBeforeResolve,
      "loading must preserve the previous breakdown, including a settled empty result");
    await page.locator("#date").fill("2026-09-23");
    await page.locator("#date").dispatchEvent("change");
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    release();
    await page.waitForTimeout(150);
    assert.equal(await page.locator("#date").inputValue(), "2026-09-23");
    assert.equal(await page.locator("#images canvas").count(), 1, "date navigation must preserve the selected image");
    assert.equal(await page.locator("#zoom").inputValue(), "1", "choosing a date resets to the full activity day");
    await page.unroute("**/api/query/resolve_range");
    await page.locator("#date").fill("2026-09-22");
    await page.locator("#date").dispatchEvent("change");
    await page.waitForFunction(() =>
      document.querySelector("#timeline .block") && document.querySelector("#timeline").getAttribute("aria-busy") === "false",
    );
    await page.locator("#zoom").selectOption("fit");
    await page.waitForFunction(() => document.querySelector("#zoom").value === "fit"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await page.locator("#next").click();
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-23"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#zoom").inputValue(), "1", "next day resets Fit activity");
    await page.locator("#zoom").selectOption("24");
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await page.locator("#previous").click();
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-22"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#zoom").inputValue(), "1", "previous day resets zoom");
    // A delayed selection must not restore old focus or images after a later click.
    let releaseSelection;
    let selected;
    const selectedPending = new Promise((resolve) => (selected = resolve));
    let delayed = false;
    await page.route("**/api/query/selection", async (route) => {
      if (!delayed) {
        delayed = true;
        const response = await route.fetch();
        selected();
        await new Promise((resolve) => (releaseSelection = resolve));
        try {
          await route.fulfill({ response });
        } catch {}
      } else await route.continue();
    });
    const b = await timeline.boundingBox();
    const headingBeforePendingSelection = await page.locator("#selected").innerText();
    await page.mouse.click(b.x + 1, b.y + 15);
    await selectedPending;
    assert.equal(await page.locator("#selected").innerText(), headingBeforePendingSelection,
      "pending evidence must retain the timestamp of the displayed screenshot");
    await page.mouse.click(b.x + b.width * 0.9, b.y + 15);
    await page.waitForFunction(() =>
      document.querySelector("#images").textContent.includes("No screenshot"),
    );
    releaseSelection();
    await page.waitForTimeout(150);
    assert.equal(await page.locator("#images canvas").count(), 0);
    await page.unroute("**/api/query/selection");
    await page.locator("#zoom").selectOption("1");
    await page.waitForFunction(() =>
      document.querySelector("#timeline .block") && document.querySelector("#timeline").getAttribute("aria-busy") === "false",
    );
    let releaseImage;
    let imageStarted;
    let imageDelayed = false;
    const imagePending = new Promise((resolve) => (imageStarted = resolve));
    await page.route("**/api/tiles/*", async (route) => {
      if (!imageDelayed) {
        imageDelayed = true;
        const response = await route.fetch();
        imageStarted();
        await new Promise((resolve) => (releaseImage = resolve));
        try {
          await route.fulfill({ response });
        } catch {}
      } else await route.continue();
    });
    const imageBox = await timeline.boundingBox();
    await page.mouse.click(imageBox.x + 1, imageBox.y + 15);
    await imagePending;
    await page.mouse.click(imageBox.x + imageBox.width * 0.9, imageBox.y + 15);
    await page.waitForFunction(() =>
      document.querySelector("#images").textContent.includes("No screenshot"),
    );
    releaseImage();
    await page.waitForTimeout(150);
    assert.equal(await page.locator("#images canvas").count(), 0);
    await page.unroute("**/api/tiles/*");
    const committed = await page.locator("#selected").innerText();
    // An empty preview must stay visually stable while the next hover resolves.
    let releaseHover;
    let hoverStarted;
    let hoverFinished;
    const hoverPending = new Promise((resolve) => (hoverStarted = resolve));
    const hoverComplete = new Promise((resolve) => (hoverFinished = resolve));
    await page.route("**/api/query/selection", async (route) => {
      const response = await route.fetch();
      hoverStarted();
      await new Promise((resolve) => (releaseHover = resolve));
      try {
        await route.fulfill({ response });
      } finally {
        hoverFinished();
      }
    });
    const emptyBeforeHover = await page.locator("#images").innerHTML();
    const focusBeforeHover = await page.locator("#focus").innerHTML();
    const selectedCursorBeforeHover = await page.locator("#cursor").evaluate(n => n.style.left);
    await page.mouse.move(imageBox.x + imageBox.width * 0.5, imageBox.y + 15);
    await hoverPending;
    assert.equal(await page.locator("#hover-cursor").isVisible(), true);
    assert.ok(Math.abs(await page.locator("#hover-cursor").evaluate(n => parseFloat(n.style.left)) - 50) < 0.1);
    assert.equal(await page.locator("#cursor").evaluate(n => n.style.left), selectedCursorBeforeHover);
    assert.doesNotMatch(await page.locator("#timeline-tooltip").innerText(), /Loading/);
    assert.equal(await page.locator(".tooltip-preview").getAttribute("aria-busy"), "true");
    assert.doesNotMatch(await page.locator("#timeline-tooltip").innerText(), /Showing/);
    const pendingImages = await page.locator("#images").innerHTML();
    const pendingFocus = await page.locator("#focus").innerHTML();
    releaseHover();
    await hoverComplete;
    await page.unroute("**/api/query/selection");
    assert.equal(pendingImages, emptyBeforeHover, "empty screenshot preview must not flash a loading placeholder");
    assert.equal(pendingFocus, focusBeforeHover, "pending hover must not collapse the focus area");
    await page.waitForFunction(() => document.querySelector(".tooltip-preview").getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#selected").innerText(), committed, "hover must not change the large preview heading");
    await page.mouse.move(5, 5);
    assert.equal(await page.locator("#hover-cursor").isVisible(), false);
    await page.waitForFunction(
      () =>
        !document.querySelector("#selected").textContent.startsWith("Preview"),
    );
    assert.equal(await page.locator("#selected").innerText(), committed);
    // Wheel zoom stays anchored to the pointer and synchronizes the dropdown.
    const wheelBox = await timeline.boundingBox();
    const wheel = { deltaY: -120, clientX: Math.floor(wheelBox.x + wheelBox.width / 4) };
    const anchor = (wheel.clientX - wheelBox.x) / wheelBox.width;
    const zoomRequest = page.waitForRequest((request) =>
      request.url().endsWith("/api/query/overview") && request.postDataJSON().range,
    );
    await timeline.dispatchEvent("wheel", wheel);
    const zoomRange = (await zoomRequest).postDataJSON().range;
    assert.equal(await page.locator("#zoom").inputValue(), "2");
    assert.equal(zoomRange.end_ms - zoomRange.start_ms, 12 * 60 * 60 * 1000);
    assert.ok(Number.isInteger(zoomRange.start_ms));
    assert.ok(Math.abs(zoomRange.start_ms + (zoomRange.end_ms - zoomRange.start_ms) * anchor
      - (1790042400000 + 24 * 60 * 60 * 1000 * anchor)) <= 1);
    await page.waitForFunction(() => document.querySelector("#images .empty"));
    await page.waitForTimeout(200);
    const unzoomRequest = page.waitForRequest((request) =>
      request.url().endsWith("/api/query/overview") && request.postDataJSON().range,
    );
    await timeline.dispatchEvent("wheel", { ...wheel, deltaY: 120 });
    const unzoomRange = (await unzoomRequest).postDataJSON().range;
    assert.equal(await page.locator("#zoom").inputValue(), "1");
    assert.equal(unzoomRange.start_ms, 1790042400000);
    assert.equal(unzoomRange.end_ms - unzoomRange.start_ms, 24 * 60 * 60 * 1000);
    await page.waitForFunction(() => document.querySelector("#images .empty"));
    await timeline.dispatchEvent("pointerleave");
    await page.waitForFunction(() => document.querySelector("#images").getAttribute("aria-busy") === "false");
    // The live marker moves independently and disappears outside the visible day.
    await page.clock.setFixedTime(new Date(1790042400000 + 12 * 60 * 60 * 1000));
    await page.waitForFunction(() => document.querySelector("#now-cursor").style.left === "50%");
    assert.equal(await page.locator("#now-cursor").isVisible(), true);
    await page.clock.setFixedTime(new Date(1790042400000 + 24 * 60 * 60 * 1000));
    await page.waitForFunction(() => document.querySelector("#now-cursor").hidden);
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await page.locator("#layout").selectOption("grid");
    await page.waitForFunction(() => document.querySelector("#images").getAttribute("aria-busy") === "false");
    const liveBox = await timeline.boundingBox();
    await timeline.dispatchEvent("click", {
      clientX: Math.ceil(liveBox.x + liveBox.width * 3650000 / 86400000), buttons: 1,
    });
    await page.waitForFunction(() => document.querySelector("#images .empty") && document.querySelector("#images").getAttribute("aria-busy") === "false");
    const selectedBeforeRefresh = await page.locator("#selected").innerText();
    const boundsBeforeRefresh = await page.locator(".ticks").innerText();
    const appended = new Promise(resolve => lines.once("line", resolve));
    fixture.stdin.write("append\n");
    assert.equal(await appended, "appended");
    await page.waitForFunction(() => [...document.querySelectorAll("#timeline .block")]
      .some(n => n.dataset.description?.includes("Browser")), null, { timeout: 7000 });
    assert.match(await page.locator("#applications").innerText(), /Browser/);
    assert.doesNotMatch(await page.locator("#categories").innerText(), /Research\s+—/, "live updates refresh category totals");
    assert.match(await page.locator("#stretch-distribution").innerText(), /Under 10 min\s+1 stretch/,
      "live updates refresh focus metrics; the pause keeps the stretches apart");
    assert.match(await timeline.innerText(), /Screenshots · Display C/);
    assert.equal(await page.locator("#images canvas").count(), 0, "live refresh must not replace the selected preview");
    await timeline.dispatchEvent("click", { clientX: Math.ceil(liveBox.x + liveBox.width * 3650000 / 86400000), buttons: 1 });
    await page.waitForFunction(() => document.querySelectorAll("#images canvas").length === 1);
    assert.match(await page.locator("#focus").innerText(), /Added while open/);
    assert.equal(await page.locator("#selected").innerText(), selectedBeforeRefresh);
    assert.equal(await page.locator(".ticks").innerText(), boundsBeforeRefresh);
    assert.equal(await page.locator("#zoom").inputValue(), "1");
    assert.equal(await page.locator("#layout").inputValue(), "grid");
    // A delayed background refresh must neither overlap itself nor restore an old day.
    let releaseRefresh;
    let refreshStarted;
    let refreshFinished;
    let backgroundRequests = 0;
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
    assert.ok(await page.locator("#timeline .track").count() > 0);
    await page.locator("#date").fill("2026-09-23");
    await page.locator("#date").dispatchEvent("change");
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    releaseRefresh();
    await refreshComplete;
    await page.unroute("**/api/query/overview_updates");
    assert.equal(await page.locator("#date").inputValue(), "2026-09-23");
    assert.equal(await page.locator("#timeline .block").evaluateAll(nodes =>
      nodes.some(n => n.dataset.description?.includes("Added while open"))), false);
    await page.waitForFunction(() => !document.querySelector("#zoom").disabled
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    // A wheel gesture updates the scale immediately but fetches only its final range.
    let wheelReads = 0;
    const countWheelReads = request => {
      if (request.url().endsWith("/api/query/overview")) wheelReads++;
    };
    page.on("request", countWheelReads);
    const finalBox = await timeline.boundingBox();
    const wheelEvent = { clientX: finalBox.x + finalBox.width / 2, deltaY: -120 };
    await timeline.dispatchEvent("wheel", wheelEvent);
    assert.equal(await page.locator("#zoom").inputValue(), "2");
    await page.waitForTimeout(190);
    await timeline.dispatchEvent("wheel", wheelEvent);
    assert.equal(await page.locator("#zoom").inputValue(), "4");
    await page.waitForTimeout(190);
    await timeline.dispatchEvent("wheel", { ...wheelEvent, deltaY: 120 });
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    page.off("request", countWheelReads);
    assert.equal(wheelReads, 1, "wheel movement should fetch only the final range");
    assert.equal(await page.locator("#zoom").inputValue(), "2");
    const minuteRequest = page.waitForRequest(request => request.url().endsWith("/api/query/overview"));
    await page.locator("#zoom").selectOption({ label: "1 minute" });
    const minuteRange = (await minuteRequest).postDataJSON().range;
    assert.equal(minuteRange.end_ms - minuteRange.start_ms, 60_000);
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await timeline.dispatchEvent("wheel", wheelEvent);
    assert.equal(await page.locator("#zoom").inputValue(), "1440", "wheel zoom stops at one minute");

    // Drag a one-minute range left: content follows the pointer, selection stays put.
    await page.mouse.move(5, 5);
    await page.waitForFunction(() => document.querySelector("#images").getAttribute("aria-busy") === "false");
    await timeline.scrollIntoViewIfNeeded();
    const dragBox = await timeline.boundingBox();
    const selectedBeforeDrag = await timeline.getAttribute("aria-valuetext");
    const boundsBeforeDrag = await page.locator(".ticks").innerText();
    let dragReads = 0;
    const countDragReads = request => { if (request.url().endsWith("/api/query/overview")) dragReads++; };
    page.on("request", countDragReads);
    await page.mouse.move(dragBox.x + dragBox.width * 0.6, dragBox.y + 15);
    await page.mouse.down();
    await page.mouse.move(dragBox.x + dragBox.width * 0.35, dragBox.y + 15, { steps: 5 });
    assert.notEqual(await page.locator(".ticks").innerText(), boundsBeforeDrag, "drag must immediately reposition the range");
    assert.equal(await timeline.getAttribute("aria-valuetext"), selectedBeforeDrag, "drag must not select another time");
    await page.waitForTimeout(250);
    assert.equal(dragReads, 0, "hold the existing tracks until drag release");
    const dragRequest = page.waitForRequest(request => request.url().endsWith("/api/query/overview"));
    await page.mouse.up();
    const draggedRange = (await dragRequest).postDataJSON().range;
    assert.ok(Math.abs(draggedRange.start_ms - minuteRange.start_ms - 15_000) < 100,
      "drag distance must map proportionally to the visible time range");
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    page.off("request", countDragReads);
    assert.equal(dragReads, 1);
    assert.equal(await timeline.getAttribute("aria-valuetext"), selectedBeforeDrag);
    await page.mouse.move(5, 5);

    // Navigation controls select real captures, fit recorded activity and cross days.
    await page.locator("#date").fill("2026-09-22");
    await page.locator("#date").dispatchEvent("change");
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await page.locator("#zoom").selectOption("1");
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    const navBox = await timeline.boundingBox();
    await timeline.dispatchEvent("click", { clientX: navBox.x, buttons: 1 });
    await page.waitForFunction(() => document.querySelector("#images").getAttribute("aria-busy") === "false");
    const nextCapture = page.waitForRequest(request => request.url().endsWith("/api/query/selection") && request.postDataJSON().selected_ms === 1790042405000);
    await timeline.press("ArrowRight");
    await nextCapture;
    await page.waitForFunction(() => document.querySelector("#images").getAttribute("aria-busy") === "false");
    const previousCapture = page.waitForRequest(request => request.url().endsWith("/api/query/selection") && request.postDataJSON().selected_ms === 1790042400000);
    await timeline.press("ArrowLeft");
    await previousCapture;
    await page.waitForFunction(() => document.querySelector("#images").getAttribute("aria-busy") === "false");
    const beforeFitImage = await page.locator("#images").innerHTML();
    const fitQuery = page.waitForRequest(request => request.url().endsWith("/api/query/overview") && request.postDataJSON().range);
    await page.locator("#zoom").selectOption("fit");
    const fitted = (await fitQuery).postDataJSON().range;
    assert.ok(fitted.start_ms <= 1790042400000 && fitted.end_ms >= 1790042400000 + 3725000);
    assert.ok(fitted.end_ms - fitted.start_ms < 2 * 3600000);
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#zoom").inputValue(), "fit");
    assert.equal(await page.locator("#images").innerHTML(), beforeFitImage);
    await page.clock.setFixedTime(new Date(1790042400000 + 86400000 + 5000));
    await page.locator("#now").click();
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-23" && document.querySelector("#images").getAttribute("aria-busy") === "false" && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await timeline.press("ArrowLeft");
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-22" && document.querySelector("#images").getAttribute("aria-busy") === "false" && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.match(await page.locator("#focus").innerText(), /Added while open/);
    await page.locator("#zoom").selectOption("1");
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await timeline.scrollIntoViewIfNeeded();
    const crossDayBox = await timeline.boundingBox();
    const beforeCrossDayImage = await page.locator("#images").innerHTML();
    const beforeCrossDayTracks = await timeline.locator(".track").count();
    const beforeCrossDayBlocks = await timeline.locator(".block").count();
    const beforeCrossDayRanking = await page.locator("#applications").innerHTML();
    const beforeCrossDayBreakdownRange = await page.locator(".breakdown-heading p").innerText();
    let releaseCrossDay, crossDayStarted;
    let crossDayDelayed = false;
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
    await page.mouse.move(crossDayBox.x + crossDayBox.width * 0.8, crossDayBox.y + 15);
    await page.mouse.down();
    await page.mouse.move(crossDayBox.x + crossDayBox.width * 0.1, crossDayBox.y + 15, { steps: 5 });
    assert.equal(await page.locator("#applications").innerHTML(), beforeCrossDayRanking,
      "dragging must retain the loaded breakdown");
    await page.mouse.up();
    await crossDayPending;
    assert.equal(await timeline.locator(".track").count(), beforeCrossDayTracks,
      "a full-day drag across the date boundary must keep its tracks while loading");
    assert.equal(await timeline.locator(".block").count(), beforeCrossDayBlocks,
      "a full-day drag must keep previously loaded events while loading");
    assert.equal((await timeline.boundingBox()).height, crossDayBox.height,
      "the timeline must not collapse during date navigation");
    assert.equal(await page.locator("#applications").innerHTML(), beforeCrossDayRanking);
    assert.equal(await page.locator(".breakdown-heading p").innerText(),
      beforeCrossDayBreakdownRange,
      "retained totals must remain labeled with their actual source range");
    releaseCrossDay();
    await page.waitForFunction(() => document.querySelector("#date").value === "2026-09-23" && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#images").innerHTML(), beforeCrossDayImage);
    await page.unroute("**/api/query/overview");
    assert.ok(crossDayQueries.length > 0);
    assert.ok(crossDayQueries.every(args => args.range && !args.date),
      "cross-day navigation fetches the dragged range without a discarded date overview");
    assert.ok(crossDayQueries.slice(1).every(args => args.cursor),
      "one overview snapshot, with only pagination requests after it");
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
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), "navigation must fit a narrow screen");
    const mobileBox = await timeline.boundingBox();
    const helpBeforeHover = await page.locator("#hover").boundingBox();
    const viewerBeforeHover = await page.locator(".evidence-panel").boundingBox();
    await timeline.dispatchEvent("pointermove", { clientX: mobileBox.x + 5, clientY: mobileBox.y + 15, buttons: 0 });
    assert.equal((await page.locator("#hover").boundingBox()).height, helpBeforeHover.height);
    assert.equal((await page.locator(".evidence-panel").boundingBox()).y, viewerBeforeHover.y,
      "hovering must not move the page below the timeline");
    await timeline.dispatchEvent("pointerleave");
    await page.screenshot({ path: "/tmp/timeline-navigation-mobile.png", fullPage: true });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false");

    assert.deepEqual(errors, []);
    await page.screenshot({
      path: "/tmp/timeline-browser-ticket04.png",
      fullPage: true,
    });
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
    await page.waitForFunction(() => document.querySelectorAll("#commit-track .commit-marker").length
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    // Commits at 04:10:00 and 04:10:30 share a marker at full-day scale; 04:35 and 04:55 stand apart.
    const markers = page.locator("#commit-track .commit-marker");
    assert.equal(await markers.count(), 3);
    assert.equal(await markers.first().innerText(), "2");
    assert.match(await markers.first().getAttribute("aria-label"), /^2 commits: timeline: Plan the commit track \([0-9a-f]{7}, 04:10:00\); timeline: Fix marker spacing/);
    assert.match(await page.locator(".commit-label").innerText(), /Commits · 4 in view[\s\S]*1 repository or path could not be read/);
    await markers.first().hover();
    const tooltip = page.locator("#timeline-tooltip");
    assert.match(await tooltip.innerText(), /Plan the commit track\s+timeline · main · [0-9a-f]{7} · 04:10:00[\s\S]*Fix marker spacing/);

    // One tab stop: arrows move between markers, Enter selects the commit's moment.
    await page.mouse.move(0, 0);
    await markers.first().focus();
    assert.match(await tooltip.innerText(), /Plan the commit track/, "focus shows the tooltip");
    await page.keyboard.press("ArrowRight");
    assert.equal(await page.evaluate(() => document.activeElement.getAttribute("aria-label")).then(label => label.startsWith("Commit notes: Notes on markers")), true);
    assert.deepEqual(await markers.evaluateAll(nodes => nodes.map(node => node.tabIndex)), [-1, 0, -1]);
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => document.querySelector("#selected")?.textContent.includes("04:35:00"));

    // Zooming in on the cluster separates its commits.
    await page.locator("#zoom").selectOption("fit");
    await page.waitForFunction(() => document.querySelector("#zoom").value === "fit"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await markers.first().hover();
    await page.mouse.wheel(0, -100);
    await page.waitForFunction(() => document.querySelector("#timeline").getAttribute("aria-busy") === "false"
      && document.querySelectorAll("#commit-track .commit-marker").length >= 2 && !document.querySelector("#commit-track .cluster"));

    // The breakdown lists the loaded range's commits; each selects its moment.
    await page.locator("summary", { hasText: "Commits (" }).click();
    assert.match(await page.locator("#commit-list").innerText(), /04:10:00\s+Plan the commit track\s+timeline · main · [0-9a-f]{7}/);

    // The track's problem opens the repository details with the unreadable one.
    await page.locator(".commit-problem").click();
    await page.waitForFunction(() => document.querySelector("#repositories").open
      && document.querySelector("#repositories-found"));
    assert.match(await page.locator("#repositories").innerText(), /2 repositories in[\s\S]*broken · broken \.git file/);
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
    await page.waitForFunction(() => document.querySelector("#period-totals"));
    assert.equal(new URL(page.url()).search, "?view=week&date=2026-09-21");
    assert.equal(await page.locator(".view-switch [aria-current=page]").innerText(), "Week");
    assert.match(await page.locator("#period-heading").innerText(), /Week of 21–27 Sept 2026/);
    assert.match(await page.title(), /Week of 21–27 Sept 2026/);
    assert.equal(await page.locator("#images").count(), 0, "the day view is not loaded behind a week");

    // One hour of Planning on Tuesday; Monday has no evidence; Thursday onwards has not begun.
    await page.waitForFunction(() => !document.querySelector("#period-focus").textContent.includes("Comparing"));
    assert.match(await page.locator("#period-focus").innerText(), /1h[\s\S]*No evidence last week/);
    assert.match(await page.locator("#period-evidence").innerText(), /1 of 3 days with evidence \(none on Mon 21 Sept, Wed 23 Sept\)[\s\S]*Last week: no evidence/);
    const states = await page.locator(".day-column").evaluateAll(columns => columns.map(column => column.className.replace("day-column ", "")));
    assert.deepEqual(states, ["missing", "recorded", "missing today", "future", "future", "future", "future"]);
    assert.match(await page.locator(".day-column").nth(1).getAttribute("aria-label"), /Tue 22 Sept: 1h recorded focus, Editor 1h/);
    await page.locator("#color-by label", { hasText: /^Category$/ }).click();
    assert.match(await page.locator(".daily-chart .legend").innerText(), /Planning/);
    assert.match(await page.locator("#subject-comparison").innerText(), /Planning\s+1h/);
    assert.match(await page.locator(".day-column .segment").first().getAttribute("style"), /var\(--series-1\)/,
      "categories keep their palette slot in every view");
    await page.locator("#color-by label", { hasText: "Application" }).click();
    await page.locator(".day-column").nth(1).hover();
    assert.match(await page.locator("#daily-tooltip").innerText(), /1h\s+Editor[\s\S]*1h\s+Recorded focus/);
    // Commits per day: beneath the columns, in the tooltip and in the table.
    await page.waitForFunction(() => document.querySelector("#commit-row"));
    assert.deepEqual(await page.locator("#commit-row > span").allInnerTexts(), ["", "4", "1", "", "", "", ""]);
    assert.match(await page.locator("#daily-tooltip").innerText(), /4 commits · timeline 3, notes 1/);
    assert.match(await page.locator(".day-column").nth(2).getAttribute("aria-label"), /, 1 commit\. Open day\.$/, "a day without focus evidence can still have commits");
    assert.match(await page.locator("#period-commits").innerText(), /5 commits this week[\s\S]*1 repository or path could not be read/);
    await page.locator("summary", { hasText: "Daily totals table" }).click();
    assert.match(await page.locator("#daily-table tbody tr").nth(1).innerText(), /Tue 22 Sept\s+1h\s+0 min\s+1h\s+1h[\s\S]*\s4$/);
    const heatRow = page.locator(".heatmap-row").nth(2);
    assert.match(await heatRow.locator(".heat-cell").first().getAttribute("aria-label"), /Tue 22 Sept 04:00–05:00: 1h active focus/);

    // Keyboard: arrows move between days, Shift+arrows change week, Home returns.
    await page.locator(".day-column").nth(1).focus();
    await page.keyboard.press("ArrowRight");
    assert.equal(await page.evaluate(() => document.activeElement.getAttribute("aria-label")).then(label => label.startsWith("Wed 23 Sept")), true);
    await page.keyboard.press("Shift+ArrowLeft");
    await page.waitForFunction(() => location.search === "?view=week&date=2026-09-14"
      && document.querySelector(".period-panel").getAttribute("aria-busy") === "false");
    assert.equal(await page.locator("#period-current").isEnabled(), true);
    await page.locator(".day-column").first().focus();
    await page.keyboard.press("Home");
    await page.waitForFunction(() => location.search === "?view=week&date=2026-09-21");

    // A heatmap hour opens its day with the hour marked; Back returns to the week.
    await page.waitForFunction(() => document.querySelector(".period-panel").getAttribute("aria-busy") === "false");
    await heatRow.locator(".heat-cell").first().click();
    await page.waitForFunction(() => document.querySelector("#stretch-highlight") && document.querySelector("#date")?.value === "2026-09-22");
    assert.equal(new URL(page.url()).search, "?date=2026-09-22");
    assert.equal(await page.evaluate(() => document.activeElement.id), "timeline");
    await page.goBack();
    await page.waitForFunction(() => location.search === "?view=week&date=2026-09-21" && document.querySelector("#period-totals"));
    await page.locator(".day-column").nth(1).click();
    await page.waitForFunction(() => document.querySelector("#date")?.value === "2026-09-22"
      && document.querySelector("#timeline").getAttribute("aria-busy") === "false");
    await page.locator("#next").click();
    await page.waitForFunction(() => location.search === "?date=2026-09-23", "day navigation keeps the URL current");
    await page.reload();
    await page.waitForFunction(() => document.querySelector("#date")?.value === "2026-09-23");

    await page.locator(".view-switch a", { hasText: "Month" }).click();
    await page.waitForFunction(() => location.search === "?view=month&date=2026-09-01" && document.querySelector("#period-totals"));
    assert.match(await page.locator("#period-heading").innerText(), /September 2026/);
    assert.equal(await page.locator(".day-column").count(), 30);
    assert.equal(await page.locator(".heatmap-row").count(), 1 + 23, "hour labels and days that have begun");
    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(200);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), "the month fits a narrow screen");
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
    await stopFixture(fixture);
  }
});
