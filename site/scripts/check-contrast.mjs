// WCAG AA contrast check on the colour tokens, in both themes.
// Parses src/styles/tokens.css (light-dark() pairs) so the tokens stay the single source of truth.
import { readFile } from 'node:fs/promises';

const css = await readFile('src/styles/tokens.css', 'utf8');
const tokens = {};
for (const m of css.matchAll(/--([\w-]+):\s*light-dark\(\s*(#[0-9a-f]{3,8}|rgb\([^)]*\))\s*,\s*(#[0-9a-f]{3,8}|rgb\([^)]*\))\s*\)/gi)) tokens[m[1]] = { light: m[2], dark: m[3] };
for (const m of css.matchAll(/--([\w-]+):\s*(#[0-9a-f]{3,8})\s*;/gi)) tokens[m[1]] ??= { light: m[2], dark: m[2] };

const rgb = (c) => {
  if (c.startsWith('#')) { let h = c.slice(1); if (h.length === 3) h = [...h].map((x) => x + x).join(''); return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)); }
  return c.match(/[\d.]+/g).slice(0, 3).map(Number);
};
const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
const mix = (fg, bg, a) => fg.map((v, i) => v * a + bg[i] * (1 - a));
const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
const get = (n, theme) => rgb(tokens[n]?.[theme] ?? n);

// [foreground, background, minimum ratio, note, foreground opacity]
const pairs = [
  ['ink', 'bg', 4.5, 'body text'], ['ink', 'bg', 4.5, 'inactive step label (62% ink)', 0.62], ['ink', 'surface', 4.5, 'text on cards'],
  ['muted', 'bg', 4.5, 'secondary text'], ['muted', 'surface', 4.5, 'secondary text on cards'],
  ['accent-text', 'bg', 4.5, 'links, chips, small accent text'], ['accent-text', 'surface', 4.5, 'accent text on cards'],
  ['on-accent', 'accent', 4.5, 'primary button label'],
  ['accent', 'bg', 3, 'accent as UI / large display element'],
  ['paper-ink', 'paper', 4.5, 'text on paper'], ['paper-ink', 'paper', 4.5, 'illustration labels (80% ink)', 0.8],
  ['#1d2b4f', 'field', 4.5, 'typed form text'],
  ['#fbf8f1', '#1b1915', 4.5, 'tag labels'],
];
let bad = 0;
for (const theme of ['light', 'dark']) {
  for (const [fg, bg, min, note, alpha = 1] of pairs) {
    const b = get(bg, theme);
    const f = alpha < 1 ? mix(get(fg, theme), b, alpha) : get(fg, theme);
    const r = ratio(f, b);
    const ok = r >= min;
    if (!ok) bad++;
    console.log(`${ok ? 'ok  ' : 'FAIL'} ${theme.padEnd(5)} ${r.toFixed(2).padStart(5)} (min ${min})  ${fg} on ${bg}  ${note}`);
  }
}
process.exit(bad ? 1 : 0);
