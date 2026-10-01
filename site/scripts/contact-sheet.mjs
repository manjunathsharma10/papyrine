// Builds review-screenshots/index.html (every capture, grouped) and contact-sheet PNGs
// (one per size/theme/half of the scenes) so a whole pass can be reviewed at a glance.
//   node scripts/contact-sheet.mjs [animated|reduced]
import { chromium } from 'playwright';
import { writeFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const mode = process.argv[2] ?? 'animated';
const root = path.resolve('review-screenshots');
const scenes = ['hero', 'annotate', 'organise', 'fill', 'compress', 'lightweight', 'offline', 'open-source'];
const stops = mode === 'reduced' ? ['end'] : ['start', 'mid', 'end'];
const sizes = { desktop: 1440, mobile: 390 };
const cellW = { desktop: 440, mobile: 210 };

const has = async (p) => readdir(p).then(() => true, () => false);
const html = (size, theme, list) => {
  const dir = `${mode}/${size}/${theme}`;
  const rows = list.map((s) => `<tr><th>${s}</th>${stops.map((st) => `<td><img width="${cellW[size]}" src="${dir}/${s}-${st}.png" alt="${s} ${st}"></td>`).join('')}</tr>`).join('');
  return `<!doctype html><meta charset=utf-8><style>body{margin:0;background:#888;font:12px system-ui}table{border-spacing:8px}th{writing-mode:vertical-rl;color:#fff;text-align:center}img{display:block;border:1px solid #0004}</style><table>${rows}</table>`;
};

const browser = await chromium.launch({ channel: 'chrome' });
const page = await browser.newPage({ deviceScaleFactor: 1 });
const index = [];
for (const size of Object.keys(sizes)) {
  for (const theme of ['light', 'dark']) {
    if (!(await has(path.join(root, mode, size, theme)))) continue;
    const halves = [scenes.slice(0, 4), scenes.slice(4)];
    for (const [i, list] of halves.entries()) {
      const file = path.join(root, `sheet-${mode}-${size}-${theme}-${i + 1}.png`);
      const tmp = path.join(root, `_sheet.html`);
      await writeFile(tmp, html(size, theme, list));
      await page.setViewportSize({ width: stops.length * (cellW[size] + 8) + 60, height: 400 });
      await page.goto(pathToFileURL(tmp).href);
      await page.waitForLoadState('load');
      await page.screenshot({ path: file, fullPage: true });
      index.push(`<h2>${size} · ${theme} · part ${i + 1}</h2><img style="max-width:100%" src="${path.basename(file)}">`);
    }
  }
}
await writeFile(path.join(root, `index-${mode}.html`), `<!doctype html><meta charset=utf-8><title>Review screenshots</title><body style="font:16px system-ui;margin:2rem">${index.join('')}`);
await browser.close();
console.log('Contact sheets written to review-screenshots/');
