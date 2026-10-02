/**
 * Fuzzy subsequence matcher for the command palette. Rewards matches at word
 * starts and consecutive runs, penalises gaps; case and diacritic insensitive.
 */

export interface FuzzyMatch {
  score: number;
  /** Indices into the original text of the matched characters. */
  positions: number[];
}

const NEG = -Infinity;
const GAP_LEADING = -0.005;
const GAP_INNER = -0.01;
const GAP_TRAILING = -0.005;
const CONSECUTIVE = 1.0;

function foldChar(ch: string): string {
  const base = ch.normalize("NFD")[0] ?? ch;
  return base.toLowerCase();
}

function fold(s: string): string {
  let out = "";
  for (const ch of s) out += foldChar(ch);
  return out;
}

function boundaryBonus(text: string, j: number): number {
  if (j === 0) return 0.8;
  const prev = text[j - 1] as string;
  const cur = text[j] as string;
  if (/[\s\-_./:()]/.test(prev)) return 0.8;
  if (prev === prev.toLowerCase() && cur !== cur.toLowerCase() && /\p{L}/u.test(cur)) return 0.7;
  return 0;
}

export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  const q = fold(query.replace(/\s+/g, ""));
  const m = q.length;
  if (m === 0) return { score: 0, positions: [] };
  const original = Array.from(text).join("");
  const t = fold(original);
  const n = t.length;
  if (m > n) return null;

  // Quick reject: must be a subsequence.
  let qi = 0;
  for (let j = 0; j < n && qi < m; j++) if (t[j] === q[qi]) qi++;
  if (qi < m) return null;

  const bonus = Array.from({ length: n }, (_, j) => boundaryBonus(original, j));
  const D: number[][] = Array.from({ length: m }, () => new Array<number>(n).fill(NEG));
  const M: number[][] = Array.from({ length: m }, () => new Array<number>(n).fill(NEG));
  for (let i = 0; i < m; i++) {
    let prevM = NEG;
    const gap = i === m - 1 ? GAP_TRAILING : GAP_INNER;
    for (let j = 0; j < n; j++) {
      let d = NEG;
      if (q[i] === t[j]) {
        if (i === 0) d = j * GAP_LEADING + (bonus[j] as number);
        else if (j > 0) {
          const viaGap = (M[i - 1] as number[])[j - 1] as number;
          const viaRun = (D[i - 1] as number[])[j - 1] as number;
          d = Math.max(viaGap + (bonus[j] as number), viaRun + CONSECUTIVE);
        }
      }
      (D[i] as number[])[j] = d;
      prevM = Math.max(d, prevM + gap);
      (M[i] as number[])[j] = prevM;
    }
  }
  const best = (M[m - 1] as number[])[n - 1] as number;
  if (best === NEG) return null;

  // Backtrack positions.
  const positions: number[] = new Array<number>(m);
  let j = n - 1;
  for (let i = m - 1; i >= 0; i--) {
    for (; j >= 0; j--) {
      const d = (D[i] as number[])[j] as number;
      if (d !== NEG && Math.abs(d - ((M[i] as number[])[j] as number)) < 1e-9) {
        positions[i] = j;
        j--;
        break;
      }
    }
  }
  return { score: best, positions };
}

export function rankMatches<T>(
  query: string,
  items: readonly T[],
  text: (item: T) => string,
  extra?: (item: T) => string,
): { item: T; match: FuzzyMatch }[] {
  if (!query.trim()) return items.map((item) => ({ item, match: { score: 0, positions: [] } }));
  const out: { item: T; match: FuzzyMatch; order: number }[] = [];
  items.forEach((item, order) => {
    const m = fuzzyMatch(query, text(item));
    if (m) {
      out.push({ item, match: m, order });
    } else if (extra) {
      const e = fuzzyMatch(query, extra(item));
      // Alias hits rank below direct title hits and carry no highlight.
      if (e) out.push({ item, match: { score: e.score - 10, positions: [] }, order });
    }
  });
  out.sort((a, b) => b.match.score - a.match.score || a.order - b.order);
  return out;
}
