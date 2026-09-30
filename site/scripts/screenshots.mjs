// Captures every scene at desktop and mobile widths, in both themes, at start / middle / end of
// its scroll progress. Uses the system Chrome (no browser download).
//   npm run screenshots                       (needs a running server, default the dev server)
//   SITE_URL=http://localhost:4321/folio/ npm run screenshots
//   REDUCED=1 npm run screenshots             (prefers-reduced-motion: static end states)
import { chromium } from 'playwright';
import { mkdir, rm } from 'node:fs/promises';
import path from 'node:path';

const url = process.env.SITE_URL ?? 'http://localhost:4322/folio/';
const out = path.resolve(process.env.OUT ?? 'review-screenshots');
const reduced = process.env.REDUCED === '1';
const only = process.env.ONLY?.split(',');
const sizes = { desktop: { width: 1440, height: 900 }, mobile: { width: 390, height: 844 } };
const scenes = ['hero', 'annotate', 'organise', 'fill', 'compress', 'lightweight', 'offline', 'open-source'];
const stops = { start: 0.02, mid: 0.5, end: 0.985 };

await rm(out, { recursive: true, force: true });
const browser = await chromium.launch({ channel: 'chrome' });

for (const [sizeName, viewport] of Object.entries(sizes)) {
  for (const theme of ['light', 'dark']) {
    const ctx = await browser.newContext({
      viewport, colorScheme: theme, deviceScaleFactor: sizeName === 'mobile' ? 2 : 1,
      reducedMotion: reduced ? 'reduce' : 'no-preference', isMobile: sizeName === 'mobile', hasTouch: sizeName === 'mobile',
    });
    const page = await ctx.newPage();
    await page.goto(url, { waitUntil: 'networkidle' });
    await page.evaluate(() => document.fonts.ready);
    const dir = path.join(out, reduced ? 'reduced' : 'animated', sizeName, theme);
    await mkdir(dir, { recursive: true });

    for (const id of scenes.filter((s) => !only || only.includes(s))) {
      const pts = reduced ? { end: 0 } : stops;
      for (const [label, p] of Object.entries(pts)) {
        const y = await page.evaluate(({ id, p, reduced }) => {
          const sec = document.getElementById(id);
          const vh = innerHeight;
          const tl = sec.matches('[data-tl]') ? sec : sec.querySelector('[data-tl]');
          const abs = (el) => el.getBoundingClientRect().top + scrollY;
          if (reduced) return abs(sec) - 56;
          const cs = getComputedStyle(tl);
          if (tl.dataset.mode === 'pin') return abs(tl) + p * (tl.offsetHeight - vh);
          const rs = parseFloat(cs.getPropertyValue('--rs')), rw = parseFloat(cs.getPropertyValue('--rw'));
          const cover = (rs + p * rw) / 100;
          // (vh - top) / (vh + h) = cover  =>  top = vh - cover * (vh + h)
          return abs(tl) - (vh - cover * (vh + tl.offsetHeight));
        }, { id, p, reduced });
        await page.evaluate((y) => scrollTo(0, y), y);
        await page.waitForTimeout(450);
        await page.screenshot({ path: path.join(dir, `${id}-${label}.png`) });
      }
    }
    await ctx.close();
  }
}
await browser.close();
console.log(`Screenshots written to ${out}`);
