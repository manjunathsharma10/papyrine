// Layout guard: at a spread of viewport sizes, (1) nothing forces horizontal scroll and
// (2) every pinned scene's content fits inside its 100svh sticky stage (nothing is clipped).
//   SITE_URL=http://localhost:4321/papyrine/ node scripts/check-layout.mjs
import { chromium } from 'playwright';

const url = process.env.SITE_URL ?? 'http://localhost:4322/papyrine/';
const viewports = [[390, 844], [375, 667], [360, 740], [430, 932], [768, 1024], [1024, 768], [1280, 720], [1280, 600], [1366, 768], [834, 1112], [1440, 900], [1920, 1080]];
const browser = await chromium.launch({ channel: 'chrome' });
let failures = 0;
for (const [width, height] of viewports) {
  const ctx = await browser.newContext({ viewport: { width, height }, reducedMotion: 'no-preference' });
  const page = await ctx.newPage();
  await page.goto(url, { waitUntil: 'networkidle' });
  const res = await page.evaluate(() => {
    const out = { hscroll: document.documentElement.scrollWidth - innerWidth, clipped: [] };
    document.querySelectorAll('.scene[data-mode="pin"] .stage').forEach((st) => {
      const over = st.scrollHeight - st.clientHeight;
      if (over > 1) out.clipped.push(`${st.closest('section').id} (+${over}px)`);
    });
    return out;
  });
  const bad = res.hscroll > 0 || res.clipped.length;
  if (bad) failures++;
  console.log(`${bad ? 'FAIL' : 'ok  '} ${width}x${height}  hscroll=${res.hscroll}  clipped=${res.clipped.join(', ') || '-'}`);
  await ctx.close();
}
await browser.close();
process.exit(failures ? 1 : 0);
