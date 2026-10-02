import { expect, test } from "@playwright/test";
import { MOD, boot, currentPage, openSample, zoomPercent } from "./helpers";

const press = (page: import("@playwright/test").Page, k: string) => page.keyboard.press(k);

test.describe("keyboard-only flows", () => {
  test.beforeEach(async ({ page }) => {
    await boot(page);
    await openSample(page);
  });

  test("open: document tab, viewer focus, first tile painted", async ({ page }) => {
    await expect(page.getByTestId("doc-tab")).toHaveText(/sample-1\.pdf/);
    await expect(page.getByTestId("viewer")).toBeFocused();
    expect(await currentPage(page)).toBe(1);
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 24");
  });

  test("navigate pages: next/prev/first/last, go to page, back/forward", async ({ page }) => {
    await press(page, `${MOD}+ArrowDown`);
    await expect(page.getByTestId("status-page")).toHaveText("Page 2 of 24");
    await press(page, `${MOD}+ArrowUp`);
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 24");
    await press(page, `${MOD}+End`);
    await expect(page.getByTestId("status-page")).toHaveText("Page 24 of 24");
    await press(page, `${MOD}+Home`);
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 24");

    // Go to page dialog.
    await press(page, `${MOD}+g`);
    await expect(page.getByRole("dialog", { name: "Go to page" })).toBeVisible();
    await page.keyboard.type("10");
    await press(page, "Enter");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByTestId("status-page")).toHaveText("Page 10 of 24");
    // The viewer really scrolled there and rendered that page's tiles.
    await expect(page.locator('.page[data-page="9"] canvas[data-ready="1"]').first()).toBeVisible();

    // History: back returns to page 1 (where we jumped from), forward returns to 10.
    await press(page, "Alt+ArrowLeft");
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 24");
    await press(page, "Alt+ArrowRight");
    await expect(page.getByTestId("status-page")).toHaveText("Page 10 of 24");
  });

  test("scrolling updates the page indicator; Home/End in the viewer", async ({ page }) => {
    const viewer = page.getByTestId("viewer");
    await viewer.evaluate((el) => (el.scrollTop = el.scrollHeight / 2));
    await expect.poll(() => currentPage(page)).toBeGreaterThan(8);
    await viewer.focus();
    await press(page, "End");
    await expect(page.getByTestId("status-page")).toHaveText("Page 24 of 24");
    await press(page, "Home");
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 24");
  });

  test("zoom: shortcuts, presets, typed value, wheel, limits", async ({ page }) => {
    await press(page, `${MOD}+1`);
    await expect(page.getByTestId("status-zoom")).toHaveText("100%");
    await press(page, `${MOD}+Equal`);
    await expect(page.getByTestId("status-zoom")).toHaveText("125%");
    await press(page, `${MOD}+Minus`);
    await press(page, `${MOD}+Minus`);
    await expect(page.getByTestId("status-zoom")).toHaveText("75%");

    await press(page, `${MOD}+0`);
    const fitPage = await zoomPercent(page);
    await press(page, `${MOD}+2`);
    const fitWidth = await zoomPercent(page);
    expect(fitWidth).toBeGreaterThan(fitPage);

    // Typed zoom, including the extremes.
    const input = page.getByTestId("zoom-input");
    await input.focus();
    await page.keyboard.type("250");
    await press(page, "Enter");
    await expect(page.getByTestId("status-zoom")).toHaveText("250%");
    await input.fill("99999");
    await press(page, "Enter");
    await expect(page.getByTestId("status-zoom")).toHaveText("6400%");
    await input.fill("1");
    await press(page, "Enter");
    await expect(page.getByTestId("status-zoom")).toHaveText("10%");

    // Ctrl-wheel zoom.
    await page.getByTestId("viewer").focus();
    await press(page, `${MOD}+1`);
    const box = (await page.getByTestId("viewer").boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.keyboard.down("Control");
    await page.mouse.wheel(0, -240);
    await page.keyboard.up("Control");
    await expect.poll(() => zoomPercent(page)).toBeGreaterThan(100);
  });

  test("zoom keeps the point under the cursor stable", async ({ page }) => {
    const viewer = page.getByTestId("viewer");
    await press(page, `${MOD}+1`);
    await viewer.evaluate((el) => (el.scrollTop = 3000));
    // Wait for virtualisation to render a page under the viewport centre.
    await page.waitForFunction(() => {
      const el = document.querySelector<HTMLElement>('[data-testid="viewer"]')!;
      const cy = el.scrollTop + el.clientHeight / 2;
      return Array.from(document.querySelectorAll<HTMLElement>(".page")).some((p) => p.offsetTop <= cy && cy <= p.offsetTop + p.offsetHeight);
    });
    const probe = () =>
      page.evaluate(() => {
        const el = document.querySelector<HTMLElement>('[data-testid="viewer"]')!;
        const cy = el.scrollTop + el.clientHeight / 2;
        let best: { page: string; frac: number; d: number } | null = null;
        for (const p of document.querySelectorAll<HTMLElement>(".page")) {
          const mid = p.offsetTop + p.offsetHeight / 2;
          const d = Math.abs(cy - mid);
          if (!best || d < best.d) best = { page: p.dataset.page!, frac: (cy - p.offsetTop) / p.offsetHeight, d };
        }
        return best;
      });
    const before = await probe();
    expect(before).not.toBeNull();
    await press(page, `${MOD}+Equal`);
    await press(page, `${MOD}+Equal`);
    const after = await probe();
    expect(after!.page).toBe(before!.page);
    expect(Math.abs(after!.frac - before!.frac)).toBeLessThan(0.02);
  });

  test("single-page view: keys change pages", async ({ page }) => {
    await press(page, `${MOD}+k`);
    await expect(page.getByTestId("palette-input")).toBeFocused();
    await page.keyboard.type("single");
    await press(page, "Enter");
    await expect(page.getByTestId("status-view")).toHaveText("Single page");
    await expect(page.locator(".page")).toHaveCount(1);
    await expect(page.getByTestId("viewer")).toBeFocused();
    await press(page, "PageDown");
    await expect(page.getByTestId("status-page")).toHaveText("Page 2 of 24");
    await expect(page.locator(".page")).toHaveCount(1);
    await expect(page.locator('.page[data-page="1"]')).toBeVisible();
    await press(page, "PageUp");
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 24");
    await press(page, "End");
    await expect(page.getByTestId("status-page")).toHaveText("Page 24 of 24");
  });

  test("panes: F6 cycles regions, thumbnails and bookmarks are keyboard operable", async ({ page }) => {
    // F6 moves through toolbar -> tabs -> left pane -> viewer -> status.
    const seen = new Set<string>();
    for (let i = 0; i < 6; i++) {
      await press(page, "F6");
      const region = await page.evaluate(() => document.activeElement?.closest("[data-region]")?.getAttribute("data-region") ?? "none");
      seen.add(region);
    }
    for (const r of ["toolbar", "tabs", "viewer", "status"]) expect(seen, r).toContain(r);

    // Thumbnails: arrow keys navigate pages.
    const thumbs = page.getByTestId("thumbs");
    await thumbs.focus();
    await press(page, "ArrowDown");
    await press(page, "ArrowDown");
    await expect(page.getByTestId("status-page")).toHaveText("Page 3 of 24");
    await expect(thumbs.locator('[aria-selected="true"]')).toHaveAttribute("aria-label", "Page 3 of 24");

    // Switch to bookmarks with the shortcut; tree navigation with arrows + Enter.
    await press(page, `${MOD}+Shift+2`);
    const tree = page.getByTestId("bookmarks");
    await expect(tree).toBeVisible();
    await tree.focus();
    await press(page, "ArrowDown"); // Section 1.1 (page 2)
    await press(page, "Enter");
    await expect(page.getByTestId("status-page")).toHaveText("Page 2 of 24");
    await expect(tree.getByRole("treeitem", { name: /Section 1\.1/ })).toHaveAttribute("aria-selected", "true");
    await press(page, "ArrowDown"); // Section 1.2 (page 4)
    await press(page, "Enter");
    await expect(page.getByTestId("status-page")).toHaveText("Page 4 of 24");
    await press(page, "ArrowDown"); // Chapter 2 (page 6)
    await press(page, "Enter");
    await expect(page.getByTestId("status-page")).toHaveText("Page 6 of 24");
    // Left collapses an open node, Right expands it again.
    const chapter2 = tree.getByRole("treeitem", { name: /Chapter 2/ });
    await expect(chapter2).toHaveAttribute("aria-expanded", "true");
    await press(page, "ArrowLeft");
    await expect(chapter2).toHaveAttribute("aria-expanded", "false");
    await expect(tree.getByRole("treeitem", { name: /Section 2\.1/ })).toHaveCount(0);
    await press(page, "ArrowRight");
    await expect(chapter2).toHaveAttribute("aria-expanded", "true");

    // Toggle the left pane away and back.
    await press(page, "F4");
    await expect(page.getByTestId("bookmarks")).toHaveCount(0);
    await press(page, "F4");
    await expect(page.getByTestId("thumbs")).toBeVisible();
  });

  test("splitter resizes with the keyboard", async ({ page }) => {
    const sep = page.getByRole("separator", { name: "Resize navigation pane" });
    await sep.focus();
    const before = Number(await sep.getAttribute("aria-valuenow"));
    await press(page, "ArrowRight");
    await expect(sep).toHaveAttribute("aria-valuenow", String(before + 16));
    await press(page, "End");
    await expect(sep).toHaveAttribute("aria-valuenow", "480");
  });

  test("search tool: type, results stream, activating a hit navigates, highlights the text layer", async ({ page }) => {
    await press(page, `${MOD}+f`);
    const input = page.getByTestId("find-input");
    await expect(input).toBeFocused();
    await page.keyboard.type("journal");
    await expect(page.getByTestId("find-status")).toContainText("matches searched 24 of 24 pages".replace("matches searched", "matches - searched"), { timeout: 10000 });
    const hits = page.getByTestId("find-hit");
    expect(await hits.count()).toBeGreaterThan(5);
    await hits.nth(3).focus();
    await press(page, "Enter");
    const target = Number((await hits.nth(3).locator(".where").textContent())!.replace(/\D/g, ""));
    await expect(page.getByTestId("status-page")).toHaveText(new RegExp(`Page ${target} of 24`));
    await expect(page.locator(".text-layer mark").first()).toBeAttached();
    // Closing the panel clears highlights.
    await press(page, `${MOD}+f`);
    await expect(page.getByTestId("right-pane")).toHaveCount(0);
    await expect(page.locator(".text-layer mark")).toHaveCount(0);
  });

  test("toolbar is one tab stop with roving arrow keys", async ({ page }) => {
    await page.getByTestId("viewer").focus();
    await press(page, "F6"); // status
    await press(page, "F6"); // toolbar
    const region = await page.evaluate(() => document.activeElement?.closest("[data-region]")?.getAttribute("data-region"));
    expect(region).toBe("toolbar");
    const first = await page.evaluate(() => document.activeElement?.getAttribute("aria-label"));
    await press(page, "ArrowRight");
    const second = await page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? document.activeElement?.textContent);
    expect(second).not.toBe(first);
    const tabbable = await page.locator('[role="toolbar"] [data-tb]:not([tabindex="-1"])').count();
    expect(tabbable).toBe(1);
    await press(page, "Home");
    await expect(page.getByRole("button", { name: "Open" }).first()).toBeFocused();
  });

  test("tabs: second document, Ctrl+PageDown, Delete closes, dirty close asks first", async ({ page }) => {
    await press(page, `${MOD}+o`);
    await expect(page.getByTestId("doc-tab")).toHaveCount(2);
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 10");
    await press(page, "Control+PageUp");
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 24");
    await press(page, "Control+PageDown");
    await expect(page.getByTestId("status-page")).toHaveText("Page 1 of 10");

    // Make the doc dirty via Organize, then try to close it.
    await page.getByTestId("tool-organize").click();
    await page.getByRole("button", { name: "Rotate page right" }).click();
    await expect(page.getByRole("tab", { name: /sample-2\.pdf/ })).toContainText("unsaved");
    await page.getByTestId("viewer").focus();
    await press(page, `${MOD}+w`);
    await expect(page.getByRole("dialog", { name: "Close with unsaved changes?" })).toBeVisible();
    await press(page, "Escape");
    await expect(page.getByTestId("doc-tab")).toHaveCount(2);
    // Undo makes it clean in the mock? No - undo leaves it dirty (new revision); save instead.
    await press(page, `${MOD}+s`);
    await expect(page.getByRole("tab", { name: /sample-2\.pdf/ })).not.toContainText("unsaved");
    await press(page, `${MOD}+w`);
    await expect(page.getByTestId("doc-tab")).toHaveCount(1);
    // Delete on a focused tab closes it too.
    await page.getByRole("tab").first().focus();
    await press(page, "Delete");
    await expect(page.getByTestId("welcome")).toBeVisible();
  });

  test("document properties: edit metadata through the engine command", async ({ page }) => {
    await press(page, `${MOD}+d`);
    const dlg = page.getByRole("dialog", { name: "Document properties" });
    await expect(dlg).toBeVisible();
    const title = dlg.getByLabel("Title");
    await title.fill("Quarterly numbers");
    await press(page, "Enter");
    await expect(dlg).toHaveCount(0);
    await expect(page.getByRole("tab", { name: /sample-1\.pdf/ })).toContainText("unsaved");
    await press(page, `${MOD}+d`);
    await expect(page.getByRole("dialog").getByLabel("Title")).toHaveValue("Quarterly numbers");
  });

  test("host events: repaired banner, file changed on disk, blocked action prompt", async ({ page }) => {
    await page.evaluate(() => (window as any).__papyrine.store.getState().openSources([{ kind: "path", path: "mock://repaired.pdf?pages=3" }]));
    await expect(page.getByTestId("banner-repaired")).toContainText("repaired");
    await page.getByTestId("banner-repaired").getByRole("button", { name: "Dismiss notice" }).click();
    await expect(page.getByTestId("banner-repaired")).toHaveCount(0);

    await page.evaluate(() => {
      const p = (window as any).__papyrine;
      p.host.simulateExternalChange(p.store.getState().activeId);
    });
    await expect(page.getByTestId("banner-changed-on-disk")).toContainText("changed on disk");
    await page.getByRole("button", { name: "Reload" }).click();
    await expect(page.getByTestId("banner-changed-on-disk")).toHaveCount(0);

    await page.evaluate(() => {
      const p = (window as any).__papyrine;
      p.store.getState().setPrompt({ docId: p.store.getState().activeId, kind: "uri", target: "https://example.com/" });
    });
    await expect(page.getByRole("dialog", { name: "Link blocked" })).toBeVisible();
    await press(page, "Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("errors surface as a localised toast", async ({ page }) => {
    await page.evaluate(() => (window as any).__papyrine.store.getState().openSources([{ kind: "path", path: "mock://missing.pdf" }]));
    await expect(page.locator(".toast[data-kind='error']")).toContainText("File not found");
  });
});
