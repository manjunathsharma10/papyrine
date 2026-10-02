#!/usr/bin/env node
// Reports gzip sizes of the production bundle and fails if the initial load
// (scripts + styles referenced by index.html) exceeds the budget (ARCHITECTURE 1.1: ~200 KB).
//   pnpm build && node e2e/bundle-report.mjs [--budget 200]
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const dist = resolve(fileURLToPath(new URL("../dist", import.meta.url)));
const budgetKb = Number(process.argv.includes("--budget") ? process.argv[process.argv.indexOf("--budget") + 1] : 200);
const gz = (p) => gzipSync(readFileSync(p), { level: 9 }).length;

const html = readFileSync(join(dist, "index.html"), "utf8");
const initial = new Set([...html.matchAll(/(?:src|href)="\/?(assets\/[^"]+)"/g)].map((m) => m[1]));

const rows = [];
(function walk(dir) {
  for (const n of readdirSync(dir)) {
    const p = join(dir, n);
    if (statSync(p).isDirectory()) walk(p);
    else if (/\.(js|css)$/.test(n)) rows.push({ file: p.slice(dist.length + 1), raw: statSync(p).size, gz: gz(p) });
  }
})(join(dist, "assets"));

rows.sort((a, b) => b.gz - a.gz);
const kb = (n) => (n / 1024).toFixed(1).padStart(7);
let initialGz = 0;
let totalGz = 0;
console.log("file".padEnd(44), "raw KB".padStart(8), "gzip KB".padStart(8), " load");
for (const r of rows) {
  const isInitial = initial.has(r.file);
  if (isInitial) initialGz += r.gz;
  totalGz += r.gz;
  console.log(r.file.padEnd(44), kb(r.raw), kb(r.gz), isInitial ? " initial" : " lazy");
}
console.log(`\ninitial (JS+CSS, gzip): ${(initialGz / 1024).toFixed(1)} KB   budget: ${budgetKb} KB`);
console.log(`all chunks (gzip):      ${(totalGz / 1024).toFixed(1)} KB`);
if (initialGz / 1024 > budgetKb) {
  console.error("initial bundle over budget");
  process.exit(1);
}
