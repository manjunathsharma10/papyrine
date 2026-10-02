/**
 * Pseudo-localisation generated from the English catalog (no hand-written
 * translations). ICU syntax is preserved: only literal text is transformed,
 * never argument names, plural/select keywords or selectors.
 */

const ACCENTS: Record<string, string> = {
  a: "à", b: "ƀ", c: "ç", d: "ð", e: "é", f: "ƒ", g: "ĝ", h: "ĥ", i: "í", j: "ĵ", k: "ķ", l: "ļ", m: "ɱ",
  n: "ñ", o: "ö", p: "þ", q: "ǫ", r: "ŕ", s: "š", t: "ţ", u: "ü", v: "ṽ", w: "ŵ", x: "ẋ", y: "ý", z: "ž",
  A: "À", B: "Ɓ", C: "Ç", D: "Ð", E: "É", F: "Ƒ", G: "Ĝ", H: "Ĥ", I: "Í", J: "Ĵ", K: "Ķ", L: "Ļ", M: "Ḿ",
  N: "Ñ", O: "Ö", P: "Þ", Q: "Ǫ", R: "Ŕ", S: "Š", T: "Ţ", U: "Ü", V: "Ṽ", W: "Ŵ", X: "Ẋ", Y: "Ý", Z: "Ž",
};

function accent(text: string): string {
  let out = "";
  for (const ch of text) out += ACCENTS[ch] ?? ch;
  return out;
}

/**
 * Walk an ICU message and map each run of literal text through `fn`.
 * Literal text is anything at brace depth 0 and the bodies of plural/select
 * branches. Quoted literals ('{') are copied verbatim.
 */
export function mapIcuLiterals(message: string, fn: (text: string) => string): string {
  let out = "";
  let buf = "";
  // Each open brace: "arg" (simple), "complex" (plural/select header) or "branch" (translatable body).
  const stack: ("arg" | "complex" | "branch")[] = [];
  const flush = () => {
    if (buf) out += fn(buf);
    buf = "";
  };
  const translatable = () => stack.length === 0 || stack[stack.length - 1] === "branch";
  for (let i = 0; i < message.length; i++) {
    const ch = message[i] as string;
    if (ch === "'" && message[i + 1] === "{") {
      // Quoted brace: literal text, keep the quotes untouched.
      const end = message.indexOf("'", i + 2);
      if (end > 0 && translatable()) {
        flush();
        out += message.slice(i, end + 1);
        i = end;
        continue;
      }
    }
    if (ch === "{") {
      const parent = stack[stack.length - 1];
      if (parent === "complex") {
        flush();
        out += ch;
        stack.push("branch");
        continue;
      }
      if (translatable()) flush();
      // Decide simple vs complex by looking for a comma before the closing brace.
      const close = message.indexOf("}", i);
      const comma = message.indexOf(",", i);
      stack.push(comma >= 0 && (close < 0 || comma < close) && translatable() ? "complex" : "arg");
      out += ch;
      continue;
    }
    if (ch === "}") {
      flush();
      stack.pop();
      out += ch;
      continue;
    }
    if (translatable()) buf += ch;
    else out += ch;
  }
  flush();
  return out;
}

/** en-XA: accented, ~30% longer, bracketed so truncation is visible. */
export function pseudoAccent(message: string): string {
  const body = mapIcuLiterals(message, accent);
  const letters = message.replace(/\{[^}]*\}/g, "").length;
  const pad = "~".repeat(Math.max(1, Math.ceil(letters * 0.3)));
  return `[${body} ${pad}]`;
}

const RLE = "‫";
const PDF = "‬";

/**
 * ar-XB: the whole English message in one right-to-left embedding, so spacing
 * around arguments follows RTL rules and the surrounding layout must mirror.
 */
export function pseudoRtl(message: string): string {
  return `${RLE}${message}${PDF}`;
}

export function generatePseudo(
  catalog: Record<string, string>,
  fn: (m: string) => string,
): Record<string, string> {
  return Object.fromEntries(Object.entries(catalog).map(([k, v]) => [k, fn(v)]));
}
