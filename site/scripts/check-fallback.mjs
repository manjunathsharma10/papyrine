// Parity test for the scroll fallback (used by browsers without CSS scroll-driven animations).
// Renders each scene at several progress points twice in Chrome: natively, and with
// `animation-timeline` support hidden so the JS fallback drives the same keyframes. Fails if the
// frames differ by more than a small fraction of pixels.
//   SITE_URL=http://localhost:4321/papyrine/ node scripts/check-fallback.mjs
import { chromium } from 'playwright';
import pixelmatch from 'pixelmatch';
import { PNG } from 'pngjs';

const url = process.env.SITE_URL ?? 'http://localhost:4322/papyrine/';
const scenes = ['hero', 'annotate', 'organise', 'fill', 'compress', 'lightweight', 'offline', 'open-source'];
const points = [0.15, 0.4, 0.65, 0.9];
const LIMIT = 0.012; // fraction of differing pixels
const viewport = { width: 1280, height: 800 };

const browser = await chromium.launch({ channel: 'chrome' });
const open = async (fallback) => {
  const ctx = await browser.newContext({ viewport, colorScheme: 'light', reducedMotion: 'no-preference' });
  if (fallback) await ctx.addInitScript(() => {
    const orig = CSS.supports.bind(CSS);
    CSS.supports = (...a) => (/animation-timeline/.test(a.join(' ')) ? false : orig(...a));
  });
  const page = await ctx.newPage();
  await page.goto(url, { waitUntil: 'networkidle' });
  return { ctx, page };
};
const native = await open(false);
const fb = await open(true);
const mode = await fb.page.evaluate(() => document.documentElement.className);
if (!mode.includes('sfb')) throw new Error(`fallback mode not active (html.${mode})`);

const targetY = (id, p) => native.page.evaluate(({ id, p }) => {
  const sec = document.getElementById(id);
  const tl = sec.matches('[data-tl]') ? sec : sec.querySelector('[data-tl]');
  const vh = innerHeight, abs = tl.getBoundingClientRect().top + scrollY;
  if (tl.dataset.mode === 'pin') return abs + p * (tl.offsetHeight - vh);
  const cs = getComputedStyle(tl), rs = parseFloat(cs.getPropertyValue('--rs')), rw = parseFloat(cs.getPropertyValue('--rw'));
  return abs - (vh - ((rs + p * rw) / 100) * (vh + tl.offsetHeight));
}, { id, p });

let failed = 0;
for (const id of scenes) {
  for (const p of points) {
    const y = await targetY(id, p);
    const shots = [];
    for (const { page } of [native, fb]) {
      await page.evaluate((y) => scrollTo(0, y), y);
      await page.waitForTimeout(400);
      shots.push(PNG.sync.read(await page.screenshot({ timeout: 120_000 })));
    }
    const { width, height } = shots[0];
    const diff = pixelmatch(shots[0].data, shots[1].data, null, width, height, { threshold: 0.12 }) / (width * height);
    const bad = diff > LIMIT;
    if (bad) failed++;
    console.log(`${bad ? 'FAIL' : 'ok  '} ${id.padEnd(12)} p=${p}  diff=${(diff * 100).toFixed(2)}%`);
  }
}
await browser.close();
process.exit(failed ? 1 : 0);
