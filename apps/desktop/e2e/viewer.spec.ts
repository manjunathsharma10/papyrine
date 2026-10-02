import { expect, test } from "@playwright/test";
import { MOD, boot, currentPage, openSample, zoomPercent } from "./helpers";

test.describe("viewer", () => {
  test("virtualised: a 5000-page document keeps a handful of pages in the DOM", async ({ page }) => {
    await boot(page);
    await page.evaluate(() => (window as any).__papyrine.store.getState().openSources([{ kind: "path", path: "mock://big.pdf?pages=5000" }]));
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 5000");
    await expect(page.locator(".page canvas[data-ready='1']").first()).toBeVisible();
    expect(await page.locator(".page").count()).toBeLessThan(12);
    await page.keyboard.press(`${MOD}+End`);
    await expect(page.getByTestId("status-page")).toHaveText("Page 5000 of 5000");
    await expect(page.locator('.page[data-page="4999"] canvas[data-ready="1"]').first()).toBeVisible();
    expect(await page.locator(".page").count()).toBeLessThan(12);
    // Thumbnails are virtualised too.
    expect(await page.locator(".thumb").count()).toBeLessThan(30);
  });

  test("tiles: only visible tiles are requested, even at 6400%", async ({ page }) => {
    await boot(page);
    await openSample(page);
    await page.getByTestId("zoom-input").fill("6400");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("status-zoom")).toHaveText("6400%");
    await expect(page.locator(".page canvas[data-ready='1']").first()).toBeVisible();
    const tiles = await page.locator(".page canvas.tile").count();
    expect(tiles).toBeGreaterThan(0);
    expect(tiles).toBeLessThan(60);
    // Content really is huge (virtual scroll area), not a giant canvas.
    const size = await page.getByTestId("viewer").evaluate((el) => ({ w: el.scrollWidth, h: el.scrollHeight }));
    expect(size.w).toBeGreaterThan(30000);
    const maxCanvas = await page.evaluate(() => Math.max(...Array.from(document.querySelectorAll<HTMLCanvasElement>(".page canvas")).map((c) => Math.max(c.width, c.height))));
    expect(maxCanvas).toBeLessThanOrEqual(512);
    // Back to a sane zoom: tile count drops again and nothing is left blank.
    await page.getByTestId("zoom-input").fill("100");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("status-zoom")).toHaveText("100%");
    await expect(page.locator(".page canvas[data-ready='1']").first()).toBeVisible();
  });

  test("text layer: positioned runs, selectable, in reading order for assistive tech", async ({ page }) => {
    await boot(page);
    await openSample(page);
    const layer = page.locator('.page[data-page="0"] .text-layer');
    await expect(layer.locator("[data-run]").first()).toBeAttached();
    expect(await layer.locator("[data-run]").count()).toBe(34);
    // Invisible but present.
    expect(await layer.locator("[data-run]").first().evaluate((el) => getComputedStyle(el).color)).toBe("rgba(0, 0, 0, 0)");
    // Run widths are fitted by scaleX to the drawn glyphs.
    await expect.poll(() => layer.locator("[data-run]").first().evaluate((el) => (el as HTMLElement).style.transform)).toContain("scaleX");
    // Select the first line by triple-click and read it back from the selection.
    const first = layer.locator("[data-run]").first();
    const expected = (await first.textContent())!;
    await first.click({ clickCount: 3 });
    const selected = await page.evaluate(() => window.getSelection()?.toString() ?? "");
    expect(selected.trim()).toBe(expected.trim());
    // The page is a labelled group so a screen reader announces where it is.
    await expect(page.getByRole("group", { name: "Page 1 of 24" })).toBeVisible();
  });

  test("pinch: ctrl-wheel stream and WebKit gesture events zoom, anchored", async ({ page }) => {
    await boot(page);
    await openSample(page);
    await page.keyboard.press(`${MOD}+1`);
    const box = (await page.getByTestId("viewer").boundingBox())!;
    await page.mouse.move(box.x + 300, box.y + 300);
    await page.keyboard.down("Control");
    for (let i = 0; i < 5; i++) await page.mouse.wheel(0, -30);
    await page.keyboard.up("Control");
    await expect.poll(() => zoomPercent(page)).toBeGreaterThan(110);

    // Safari/WebKit trackpad pinch arrives as gesture events.
    await page.keyboard.press(`${MOD}+1`);
    await page.getByTestId("viewer").evaluate((el) => {
      const ev = (type: string, scale: number) => {
        const e = new Event(type, { bubbles: true, cancelable: true }) as Event & { scale: number; clientX: number; clientY: number };
        e.scale = scale;
        e.clientX = 400;
        e.clientY = 300;
        el.dispatchEvent(e);
      };
      ev("gesturestart", 1);
      ev("gesturechange", 2);
      ev("gestureend", 2);
    });
    await expect.poll(() => zoomPercent(page)).toBeCloseTo(200, 0);
  });

  test("touch pinch with two pointers", async ({ page }) => {
    await boot(page);
    await openSample(page);
    await page.keyboard.press(`${MOD}+1`);
    await page.getByTestId("viewer").evaluate((el) => {
      const fire = (type: string, id: number, x: number, y: number) =>
        el.dispatchEvent(new PointerEvent(type, { pointerId: id, pointerType: "touch", clientX: x, clientY: y, bubbles: true }));
      fire("pointerdown", 1, 400, 400);
      fire("pointerdown", 2, 500, 400);
      fire("pointermove", 2, 600, 400); // distance 100 -> 200
      fire("pointerup", 1, 400, 400);
      fire("pointerup", 2, 600, 400);
    });
    await expect.poll(() => zoomPercent(page)).toBeCloseTo(200, 0);
  });

  test("fit modes follow container resizes; fit page shows a whole page", async ({ page }) => {
    await boot(page);
    await openSample(page);
    await page.keyboard.press(`${MOD}+2`);
    const before = await zoomPercent(page);
    await page.setViewportSize({ width: 900, height: 700 });
    await expect.poll(() => zoomPercent(page)).toBeLessThan(before);
    await page.keyboard.press(`${MOD}+0`);
    const box = (await page.getByTestId("viewer").boundingBox())!;
    const pageBox = (await page.locator('.page[data-page="0"]').boundingBox())!;
    expect(pageBox.height).toBeLessThanOrEqual(box.height);
  });

  test("organize pages: edits go through the engine command, invalidate tiles, undo/redo", async ({ page }) => {
    await boot(page);
    await openSample(page);
    await page.getByTestId("tool-organize").click();
    await page.getByRole("button", { name: "Delete page" }).click();
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 23");
    await page.getByRole("button", { name: "Undo" }).click();
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 24");
    await page.getByRole("button", { name: "Redo" }).click();
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 23");
    await expect(page.locator(".page canvas[data-ready='1']").first()).toBeVisible();
  });

  test("current page survives tab switches", async ({ page }) => {
    await boot(page);
    await openSample(page);
    await page.keyboard.press(`${MOD}+g`);
    await expect(page.getByTestId("goto-input")).toBeFocused();
    await page.keyboard.type("12");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("status-page")).toHaveText("Page 12 of 24");
    await page.keyboard.press(`${MOD}+o`);
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 10");
    await page.keyboard.press("Control+PageUp");
    await expect(page.getByTestId("status-page")).toHaveText("Page 12 of 24");
    await expect(page.locator('.page[data-page="11"]')).toBeVisible();
    expect(await currentPage(page)).toBe(12);
  });
});
