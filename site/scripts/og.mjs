// Renders public/og.png (1200x630) from the site's own fonts and tokens. Run after changing the name or tagline.
import { chromium } from 'playwright';
import { pathToFileURL } from 'node:url';
import { writeFile, rm } from 'node:fs/promises';
import path from 'node:path';
import { product } from '../src/config.ts';

const font = (f) => pathToFileURL(path.resolve('src/assets/fonts', f)).href;
const html = `<!doctype html><meta charset=utf-8><style>
@font-face{font-family:D;src:url(${font('display.woff2')})}
@font-face{font-family:B;src:url(${font('body.woff2')});font-weight:100 900}
body{margin:0;width:1200px;height:630px;background:#f6f1e7;color:#1b1915;font-family:B;position:relative;overflow:hidden}
.n{position:absolute;left:80px;top:64px;font:400 44px D}
h1{position:absolute;left:80px;top:150px;width:690px;margin:0;font:400 88px/.98 D;letter-spacing:-.01em}
.s{position:absolute;left:80px;bottom:64px;font:500 24px B;color:#5a564d}
.p{position:absolute;background:#fbf8f1;border-radius:6px;width:250px;height:325px;box-shadow:0 20px 50px -20px rgba(40,30,10,.45),0 0 0 1px #d8d0bf}
.p i{position:absolute;left:24px;height:8px;border-radius:4px;background:#cfc8b8}
.hl{position:absolute;left:20px;top:78px;width:160px;height:22px;background:#ffd84d;opacity:.8;border-radius:3px}
</style><body><div class=n>${product.name}</div><h1>${product.tagline}</h1><div class=s>Open source · Pre-release</div>
<div class=p style="left:840px;top:110px;transform:rotate(-8deg)"></div>
<div class=p style="left:900px;top:150px;transform:rotate(5deg)"><i style="top:30px;width:110px;height:16px;background:#63605a"></i>${[0,1,2,3,4,5,6,7,8].map((k)=>`<i style="top:${86+k*24}px;width:${[190,180,190,140,186,170,190,110,150][k]}px"></i>`).join('')}<div class=hl></div></div></body>`;
const browser = await chromium.launch({ channel: 'chrome' });
const page = await browser.newPage({ viewport: { width: 1200, height: 630 } });
const tmp = path.resolve('og-render.tmp.html');
await writeFile(tmp, html);
await page.goto(pathToFileURL(tmp).href);
await page.evaluate(() => document.fonts.ready);
await page.screenshot({ path: 'public/og.png' });
await browser.close();
await rm(tmp);
console.log('public/og.png written');
