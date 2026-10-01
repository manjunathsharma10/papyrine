// JavaScript budget: everything the page ships (bundled modules + inline scripts), gzipped.
import { readdir, readFile } from 'node:fs/promises';
import { gzipSync } from 'node:zlib';
import path from 'node:path';

const BUDGET = 100 * 1024;
const dist = path.resolve('dist');
const walk = async (d) => (await Promise.all((await readdir(d, { withFileTypes: true })).map((e) => (e.isDirectory() ? walk(path.join(d, e.name)) : [path.join(d, e.name)])))).flat();

let total = 0;
for (const f of await walk(dist)) {
  const rel = path.relative(dist, f);
  const buf = await readFile(f);
  let size = 0;
  if (f.endsWith('.js') || f.endsWith('.mjs')) size = gzipSync(buf).length;
  else if (f.endsWith('.html')) {
    const scripts = [...buf.toString().matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)].map((m) => m[1]).join('\n');
    size = scripts ? gzipSync(scripts).length : 0;
  } else continue;
  if (size) { total += size; console.log(`${String(size).padStart(6)} B  ${rel}`); }
}
console.log(`\nJavaScript total: ${(total / 1024).toFixed(2)} KB gzipped (budget ${BUDGET / 1024} KB)`);
if (total > BUDGET) { console.error('Over budget'); process.exit(1); }
