import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildPdf, dev, expect, hostCall, qpdfCheck, readFileSync, startBridge, test } from "./fixtures";
import type { DocumentInfo } from "../../src/ipc/contract";

const MOD = process.platform === "darwin" ? "Meta" : "Control";

async function openPdf(page: import("@playwright/test").Page, path: string) {
  await dev(page, "queueOpen", [path]);
  await page.getByTestId("welcome-open").focus();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("viewer")).toBeVisible();
  await expect(page.locator(".page canvas[data-ready='1']").first()).toBeVisible();
}

const active = (page: import("@playwright/test").Page) =>
  page.evaluate(() => {
    const s = (window as unknown as { __papyrine: { store: { getState(): { activeId: string; docs: Record<string, { info: DocumentInfo }> } } } }).__papyrine.store.getState();
    return s.docs[s.activeId]!.info;
  });

test("open a generated document, scroll, zoom, rotate, undo, save and reopen", async ({ page, bridge, openApp, workDir }) => {
  const pdf = join(workDir, "doc.pdf");
  const original = buildPdf(12);
  writeFileSync(pdf, original);
  await openApp(page, bridge);
  await openPdf(page, pdf);

  // The real engine and renderer answered: 12 pages with real text on page 1.
  await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 12");
  const first = page.locator('.page[data-page="0"] .text-layer [data-run]').first();
  await expect(first).toBeAttached();
  await expect.poll(() => page.locator('.page[data-page="0"] .text-layer').innerText()).toContain("Page 1");

  // Scroll: the page counter follows and the new page's tiles arrive.
  await page.keyboard.press(`${MOD}+End`);
  await expect(page.getByTestId("status-page")).toHaveText("Page 12 of 12");
  await expect(page.locator('.page[data-page="11"] canvas[data-ready="1"]').first()).toBeVisible();
  await page.keyboard.press(`${MOD}+Home`);
  await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 12");

  // Zoom.
  await page.getByTestId("zoom-input").fill("200");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("status-zoom")).toHaveText("200%");
  await expect(page.locator(".page canvas[data-ready='1']").first()).toBeVisible();
  await page.getByTestId("zoom-input").fill("100");
  await page.keyboard.press("Enter");

  // Rotate page 1 through the UI store (the same path the toolbar uses), watch the layout change.
  const before = await active(page);
  expect(before.pages[0]!.width).toBe(612);
  await page.evaluate(() =>
    (window as unknown as { __papyrine: { store: { getState(): { run(c: unknown): Promise<void> } } } }).__papyrine.store
      .getState()
      .run({ type: "rotate-pages", pages: [0], degrees: 90 }),
  );
  await expect.poll(async () => (await active(page)).pages[0]!.width).toBe(792);
  expect((await active(page)).dirty).toBe(true);
  await expect(page.locator('.page[data-page="0"] canvas[data-ready="1"]').first()).toBeVisible();

  // Undo from the keyboard, then redo.
  await page.locator("body").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press(`${MOD}+z`);
  await expect.poll(async () => (await active(page)).pages[0]!.width).toBe(612);
  await page.keyboard.press(`${MOD}+Shift+z`);
  await expect.poll(async () => (await active(page)).pages[0]!.width).toBe(792);

  // Save (Ctrl/Cmd+S): an incremental update, original bytes untouched as a prefix.
  await page.keyboard.press(`${MOD}+s`);
  await expect.poll(async () => (await active(page)).dirty).toBe(false);
  const saved = readFileSync(pdf);
  expect(saved.length).toBeGreaterThan(original.length);
  expect(saved.subarray(0, original.length).equals(original)).toBe(true);
  qpdfCheck(pdf);

  // Reopen the saved file in a second tab: the rotation is in the file.
  await page.evaluate((p) =>
    (window as unknown as { __papyrine: { store: { getState(): { openSources(s: unknown[]): Promise<void> } } } }).__papyrine.store
      .getState()
      .openSources([{ kind: "path", path: p }]), pdf);
  await expect.poll(async () => (await page.getByTestId("doc-tab").count())).toBeGreaterThanOrEqual(2);
  await expect.poll(async () => (await active(page)).pages[0]!.width).toBe(792);
});

test("search and bookmarks come from the real engine", async ({ page, bridge, openApp, workDir }) => {
  const pdf = join(workDir, "s.pdf");
  writeFileSync(pdf, buildPdf(5));
  await openApp(page, bridge);
  await openPdf(page, pdf);
  const info = await active(page);
  const job = await hostCall<string>(page, "search", info.docId, "lazy dog", { caseSensitive: false, wholeWord: false, diacriticInsensitive: true, includeComments: false, includeFormValues: false });
  expect(job).toMatch(/^job-/);
  // Use the UI's search box.
  await page.keyboard.press(`${MOD}+f`);
  await page.getByTestId("find-input").fill("lazy dog");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("find-hit").first()).toBeVisible();
  expect(await page.getByTestId("find-hit").count()).toBe(5);
});

test("engine killed mid-session: the edit continues and nothing is lost", async ({ page, bridge, openApp, workDir }) => {
  const pdf = join(workDir, "k.pdf");
  writeFileSync(pdf, buildPdf(4));
  await openApp(page, bridge);
  await openPdf(page, pdf);
  const run = (cmd: unknown) =>
    page.evaluate((c) => (window as unknown as { __papyrine: { store: { getState(): { run(c: unknown): Promise<void> } } } }).__papyrine.store.getState().run(c), cmd);
  await run({ type: "rotate-pages", pages: [0], degrees: 90 });
  await expect.poll(async () => (await active(page)).pages[0]!.width).toBe(792);
  const before = await dev<{ enginePid: number }>(page, "stats");
  await dev(page, "killEngine");
  await page.waitForTimeout(300);
  await run({ type: "rotate-pages", pages: [2], degrees: 180 });
  await expect.poll(async () => (await active(page)).pages[2]!.rotation).toBe(180);
  const info = await active(page);
  expect(info.pages[0]!.width).toBe(792);
  const after = await dev<{ enginePid: number; engineRestarts: number }>(page, "stats");
  expect(after.enginePid).not.toBe(before.enginePid);
  expect(after.engineRestarts).toBe(1);
});

test("host killed after edits: relaunch, restore, save and verify", async ({ page, bridge, openApp, workDir, dataDir, browser }) => {
  const pdf = join(workDir, "crash.pdf");
  const original = buildPdf(6);
  writeFileSync(pdf, original);
  await openApp(page, bridge);
  await openPdf(page, pdf);
  const run = (cmd: unknown) =>
    page.evaluate((c) => (window as unknown as { __papyrine: { store: { getState(): { run(c: unknown): Promise<void> } } } }).__papyrine.store.getState().run(c), cmd);
  await run({ type: "rotate-pages", pages: [0], degrees: 90 });
  await run({ type: "rotate-pages", pages: [3], degrees: 270 });
  await run({ type: "delete-pages", pages: [5] });
  await expect.poll(async () => (await active(page)).pageCount).toBe(5);

  // kill -9 the whole host (its children die with it).
  await bridge.kill9();

  const second = await startBridge(dataDir);
  try {
    const ctx = await browser.newContext();
    const page2 = await ctx.newPage();
    await page2.goto(`/?bridge=${encodeURIComponent(second.url)}`);
    await expect(page2.getByTestId("welcome")).toBeVisible();
    // The host scans for recovery after first paint.
    await expect
      .poll(() => hostCall<unknown[]>(page2, "recoveryEntries"), { timeout: 20_000 })
      .toHaveLength(1);
    const entry = (await hostCall<{ id: string; name: string; state: string }[]>(page2, "recoveryEntries"))[0]!;
    expect(entry.name).toBe("crash.pdf");
    expect(entry.state).toBe("restorable");

    const doc = (await hostCall<DocumentInfo>(page2, "recover", entry.id, "restore"))!;
    expect(doc.pageCount).toBe(5);
    expect(doc.pages[0]!.width).toBe(792);
    expect(doc.pages[3]!.rotation).toBe(270);
    expect(doc.dirty).toBe(true);

    // Show it in the UI as the app would, then save with the keyboard.
    await page2.evaluate((p) =>
      (window as unknown as { __papyrine: { store: { getState(): { openSources(s: unknown[]): Promise<void> } } } }).__papyrine.store.getState().openSources([{ kind: "path", path: p }]), pdf);
    await expect(page2.getByTestId("viewer")).toBeVisible();

    const report = await hostCall<{ status: string; info: DocumentInfo }>(page2, "save", doc.docId);
    expect(report.status).toBe("saved");
    expect(report.info.dirty).toBe(false);
    const bytes = readFileSync(pdf);
    expect(bytes.subarray(0, original.length).equals(original)).toBe(true);
    qpdfCheck(pdf);
    // Verify by opening the saved file fresh.
    const check = await hostCall<DocumentInfo>(page2, "openDocument", { kind: "path", path: pdf });
    expect(check.pageCount).toBe(5);
    expect(check.pages[0]!.width).toBe(792);
    expect(check.pages[3]!.rotation).toBe(270);
    // And the recovery entry is gone.
    expect(await hostCall<unknown[]>(page2, "recoveryEntries")).toHaveLength(0);
    await ctx.close();
  } finally {
    second.proc.kill("SIGKILL");
  }
});

test("tiles line up with the text layer at a zoom that is not a renderer bucket", async ({ page, bridge, openApp, workDir }) => {
  const pdf = join(workDir, "z.pdf");
  writeFileSync(pdf, buildPdf(2));
  await openApp(page, bridge);
  await openPdf(page, pdf);
  for (const z of ["100", "130", "250"]) {
    await page.getByTestId("zoom-input").fill(z);
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("status-zoom")).toHaveText(`${z}%`);
    await page.waitForTimeout(800);
    await expect(page.locator('.page[data-page="0"] canvas[data-ready="1"]').first()).toBeVisible();
    // Where the big "Page 1" line is drawn (ink in the tile canvases) vs where the text layer says it is.
    const r = await page.evaluate(() => {
      const pageEl = document.querySelector('.page[data-page="0"]') as HTMLElement;
      const pr = pageEl.getBoundingClientRect();
      let minX = 1e9, minY = 1e9, maxX = -1, maxY = -1;
      const run = Array.from(pageEl.querySelectorAll("[data-run]")).find((e) => e.textContent?.includes("Page 1")) as HTMLElement;
      const rb = run.getBoundingClientRect();
      const rtop = rb.top - pr.top, rbot = rb.bottom - pr.top;
      for (const c of Array.from(pageEl.querySelectorAll("canvas"))) {
        const cv = c as HTMLCanvasElement;
        const b = cv.getBoundingClientRect();
        const ctx = cv.getContext("2d");
        if (!ctx || !cv.width) continue;
        const d = ctx.getImageData(0, 0, cv.width, cv.height).data;
        const sx = b.width / cv.width, sy = b.height / cv.height;
        for (let y = 0; y < cv.height; y++) for (let x = 0; x < cv.width; x++) {
          const i = (y * cv.width + x) * 4;
          if (d[i]! < 100 && d[i + 1]! < 100 && d[i + 2]! < 100) {
            const px = b.left + x * sx - pr.left, py = b.top + y * sy - pr.top;
            // Only the title line: ink within a margin of the run's own rows.
            if (py > rtop - 12 && py < rbot + 12) { minX = Math.min(minX, px); maxX = Math.max(maxX, px); minY = Math.min(minY, py); maxY = Math.max(maxY, py); }
          }
        }
      }
      return { ink: { minX, maxX, minY, maxY }, run: { x: rb.left - pr.left, right: rb.right - pr.left, y: rb.top - pr.top, bottom: rb.bottom - pr.top } };
    });
    // Ink (glyph shapes) lies inside the run box, within a few pixels at every zoom.
    expect(r.ink.minX, `${z}%: ${JSON.stringify(r)}`).toBeGreaterThan(r.run.x - 6);
    expect(r.ink.maxX, `${z}%`).toBeLessThan(r.run.right + 6);
    expect(r.ink.minY, `${z}%`).toBeGreaterThan(r.run.y - 8);
    expect(r.ink.maxY, `${z}%`).toBeLessThan(r.run.bottom + 8);
    // ... and fills most of it (so it is not drawn somewhere else entirely).
    expect(r.ink.maxX - r.ink.minX, `${z}%`).toBeGreaterThan((r.run.right - r.run.x) * 0.8);
  }
});
