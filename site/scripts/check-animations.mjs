// Fails if any @keyframes animates a property outside the allowed set.
// Allowed: transform, opacity (compositor-friendly), and stroke-dashoffset, the one approved exception
// for the pen/signature strokes (see PLAN.md §2 and §7).
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';

const dir = path.resolve('src/styles');
const allowed = new Set(['transform', 'opacity', 'stroke-dashoffset', 'animation-timing-function']);
let bad = 0;

for (const f of (await readdir(dir)).filter((n) => n.endsWith('.css'))) {
  const css = (await readFile(path.join(dir, f), 'utf8')).replace(/\/\*[\s\S]*?\*\//g, '');
  const re = /@keyframes\s+([\w-]+)\s*\{/g;
  let m;
  while ((m = re.exec(css))) {
    let depth = 1, i = re.lastIndex;
    while (depth && i < css.length) { const c = css[i++]; if (c === '{') depth++; else if (c === '}') depth--; }
    const body = css.slice(re.lastIndex, i - 1);
    for (const decl of body.matchAll(/([\w-]+)\s*:[^;{}]+;?/g)) {
      const prop = decl[1];
      if (!allowed.has(prop)) { console.error(`${f}: @keyframes ${m[1]} animates "${prop}"`); bad++; }
    }
  }
}
if (bad) process.exit(1);
console.log('animations: only transform, opacity and stroke-dashoffset are animated');
