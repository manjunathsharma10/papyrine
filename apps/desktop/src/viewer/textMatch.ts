export interface MatchRange {
  start: number;
  length: number;
}

/** Same matching rules the search panel asks the host for (UI-side, for highlighting). */
export function findMatches(text: string, query: string, caseSensitive: boolean, wholeWord: boolean): MatchRange[] {
  if (!query) return [];
  const hay = caseSensitive ? text : text.toLowerCase();
  const q = caseSensitive ? query : query.toLowerCase();
  const out: MatchRange[] = [];
  let from = 0;
  for (;;) {
    const at = hay.indexOf(q, from);
    if (at < 0) break;
    from = at + Math.max(1, q.length);
    if (wholeWord) {
      const before = at === 0 ? " " : (hay[at - 1] as string);
      const after = hay[at + q.length] ?? " ";
      if (/\w/.test(before) || /\w/.test(after)) continue;
    }
    out.push({ start: at, length: q.length });
  }
  return out;
}
