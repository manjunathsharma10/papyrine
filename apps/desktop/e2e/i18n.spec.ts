import { expect, test, type Page } from "@playwright/test";
import { MOD, boot, openSample } from "./helpers";

const noHorizontalOverflow = (page: Page) => page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth);

/** Every toolbar control must lie inside the toolbar box (no clipping, wraps instead). */
const toolbarContained = (page: Page) =>
  page.evaluate(() => {
    const bar = document.querySelector(".toolbar")!.getBoundingClientRect();
    return Array.from(document.querySelectorAll(".toolbar > *")).every((el) => {
      const r = el.getBoundingClientRect();
      return r.left >= bar.left - 1 && r.right <= bar.right + 1;
    });
  });

test.describe("pseudo-locale (en-XA)", () => {
  test("switch from the palette; every visible string comes from the catalog", async ({ page }) => {
    await boot(page);
    await openSample(page);
    await page.keyboard.press(`${MOD}+k`);
    await expect(page.getByTestId("palette-input")).toBeFocused();
    await page.keyboard.type("accented");
    await page.keyboard.press("Enter");
    await expect(page.locator("html")).toHaveAttribute("lang", "en-XA");
    await expect(page.locator("html")).toHaveAttribute("dir", "ltr");
    await expect(page.getByTestId("status-page")).toHaveText(/^\[.*\b1\b.*\b24\b.*\]$/);
    // Walk the chrome: any text node of letters that is not bracketed is an uncatalogued string.
    const strays = await page.evaluate(() => {
      const out: string[] = [];
      const skip = new Set(["SCRIPT", "STYLE"]);
      const walker = document.createTreeWalker(document.getElementById("root")!, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const el = n.parentElement!;
        if (skip.has(el.tagName) || el.closest(".text-layer, .thumb-label, .zoom-input, .kbd, .page-no, mark")) continue;
        const txt = n.textContent!.trim();
        // File names and numbers are data, not UI strings.
        if (/\p{L}/u.test(txt) && !/^\[.*\]$/s.test(txt) && !/\.pdf$/.test(txt)) out.push(txt);
      }
      return out;
    });
    expect(strays).toEqual([]);
    expect(await noHorizontalOverflow(page)).toBe(true);
    expect(await toolbarContained(page)).toBe(true);
    await page.screenshot({ path: "e2e/artifacts/pseudo-viewer.png" });

    await page.keyboard.press(`${MOD}+f`);
    await expect(page.getByTestId("find-input")).toBeFocused();
    await page.keyboard.type("journal");
    await expect(page.getByTestId("find-hit").first()).toBeVisible();
    await page.screenshot({ path: "e2e/artifacts/pseudo-search.png" });
    await page.keyboard.press(`${MOD}+k`);
    await expect(page.getByTestId("palette-input")).toBeFocused();
    await page.screenshot({ path: "e2e/artifacts/pseudo-palette.png" });
  });

  test("narrow window: nothing overflows horizontally", async ({ page }) => {
    await page.setViewportSize({ width: 760, height: 700 });
    await boot(page, { locale: "en-XA" });
    await openSample(page);
    expect(await noHorizontalOverflow(page)).toBe(true);
    expect(await toolbarContained(page)).toBe(true);
  });
});

test.describe("RTL layout (ar-XB)", () => {
  test("chrome mirrors, document content does not", async ({ page }) => {
    await boot(page, { locale: "ar-XB" });
    await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
    await expect(page.locator("html")).toHaveAttribute("lang", "ar-XB");
    await page.screenshot({ path: "e2e/artifacts/rtl-welcome.png" });
    await openSample(page);

    const rects = await page.evaluate(() => {
      const r = (s: string) => document.querySelector(s)!.getBoundingClientRect().toJSON();
      return {
        open: r('.toolbar [data-tb]'),
        palette: r('.palette-btn'),
        leftPane: r(".left-pane"),
        main: r(".main"),
        status: r('[data-testid="status-zoom"]'),
        statusPage: r('[data-testid="status-page"]'),
        firstTab: r(".doc-tab"),
        tabbar: r(".tabbar"),
        viewerDir: getComputedStyle(document.querySelector('[data-testid="viewer"]')!).direction,
      };
    });
    // Toolbar: Open starts at the right edge, command palette ends at the left.
    expect(rects.open.x).toBeGreaterThan(rects.palette.x);
    // Navigation pane sits on the right of the document area.
    expect(rects.leftPane.x).toBeGreaterThan(rects.main.x);
    // Status bar: page indicator starts on the right, zoom on the left.
    expect(rects.statusPage.x).toBeGreaterThan(rects.status.x);
    // Tabs start from the right edge.
    expect(rects.tabbar.right - rects.firstTab.right).toBeLessThan(rects.firstTab.left - rects.tabbar.left);
    // The page itself is never mirrored.
    expect(rects.viewerDir).toBe("ltr");
    expect(await noHorizontalOverflow(page)).toBe(true);
    expect(await toolbarContained(page)).toBe(true);

    // Rendered text on a page is still left-to-right and starts at the same offset.
    const layer = page.locator('.page[data-page="0"] .text-layer [data-run]').first();
    await expect(layer).toBeAttached();
    expect(await layer.evaluate((el) => getComputedStyle(el).direction)).toBe("ltr");
    await page.screenshot({ path: "e2e/artifacts/rtl-viewer.png" });
  });

  test("keyboard: toolbar arrows and splitter follow reading direction", async ({ page }) => {
    await boot(page, { locale: "ar-XB" });
    await openSample(page);
    const open = page.locator(".toolbar [data-tb]").first();
    await open.focus();
    const firstLabel = await open.getAttribute("aria-label");
    await page.keyboard.press("ArrowLeft"); // next item in RTL
    const label2 = await page.evaluate(() => document.activeElement?.getAttribute("aria-label"));
    expect(label2).not.toBe(firstLabel);
    await page.keyboard.press("ArrowRight"); // back
    expect(await page.evaluate(() => document.activeElement?.getAttribute("aria-label"))).toBe(firstLabel);

    const sep = page.locator(".splitter").first();
    await sep.focus();
    const before = Number(await sep.getAttribute("aria-valuenow"));
    await page.keyboard.press("ArrowLeft"); // pane is on the right; moving the edge left widens it
    expect(Number(await sep.getAttribute("aria-valuenow"))).toBeGreaterThan(before);
  });

  test("tool pane docks on the left in RTL; panels and palette render", async ({ page }) => {
    await boot(page, { locale: "ar-XB" });
    await openSample(page);
    await page.keyboard.press(`${MOD}+f`);
    await expect(page.getByTestId("find-input")).toBeFocused();
    await page.keyboard.type("journal");
    await expect(page.getByTestId("find-hit").first()).toBeVisible();
    const x = await page.evaluate(() => ({ pane: document.querySelector(".right-pane")!.getBoundingClientRect().x, main: document.querySelector(".main")!.getBoundingClientRect().x }));
    expect(x.pane).toBeLessThan(x.main);
    // The pane stays inside the window (it once grew to its content's width).
    const paneBox = (await page.locator(".right-pane").boundingBox())!;
    expect(paneBox.x).toBeGreaterThanOrEqual(0);
    expect(paneBox.width).toBeLessThanOrEqual(330);
    expect(await noHorizontalOverflow(page)).toBe(true);
    await page.screenshot({ path: "e2e/artifacts/rtl-search.png" });
    await page.keyboard.press(`${MOD}+k`);
    await expect(page.getByTestId("palette-input")).toBeFocused();
    await page.screenshot({ path: "e2e/artifacts/rtl-palette.png" });
  });

  test("locale choice persists", async ({ page }) => {
    await boot(page);
    await page.getByTestId("settings-menu").click();
    await page.getByRole("menuitemradio", { name: "Pseudo-locale (right to left)" }).click();
    await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  });
});
