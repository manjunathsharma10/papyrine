import { expect, test } from "@playwright/test";
import { boot } from "./helpers";

// Indicative only: headless Chromium with software rendering and the mock host.
// The roadmap's B4 (p95 <= 16.7 ms) is measured on the real host on CI runners.
test("scroll frame times with the mock host (indicative)", async ({ page }) => {
  await boot(page);
  await page.evaluate(() => (window as any).__papyrine.store.getState().openSources([{ kind: "path", path: "mock://perf.pdf?pages=300" }]));
  await expect(page.locator(".page canvas[data-ready='1']").first()).toBeVisible();
  const stats = await page.getByTestId("viewer").evaluate(async (el) => {
    const frames: number[] = [];
    let last = performance.now();
    let done = false;
    const tick = (now: number) => {
      frames.push(now - last);
      last = now;
      if (!done) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    const start = performance.now();
    while (performance.now() - start < 3000) {
      el.scrollTop += 40;
      await new Promise((r) => requestAnimationFrame(r));
    }
    done = true;
    const sorted = frames.slice(2).sort((a, b) => a - b);
    const q = (p: number) => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))] ?? 0;
    return { n: sorted.length, p50: q(0.5), p95: q(0.95), max: sorted[sorted.length - 1] ?? 0, scrolled: el.scrollTop };
  });
  console.log(`scroll frames: n=${stats.n} p50=${stats.p50.toFixed(1)}ms p95=${stats.p95.toFixed(1)}ms max=${stats.max.toFixed(1)}ms scrolled=${Math.round(stats.scrolled)}px`);
  expect(stats.scrolled).toBeGreaterThan(1000);
  expect(stats.p95).toBeLessThan(60); // generous sanity bound, not the B4 budget
});
