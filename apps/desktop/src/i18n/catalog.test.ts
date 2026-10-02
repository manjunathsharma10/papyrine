import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { IntlMessageFormat } from "intl-messageformat";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import { COMMANDS } from "../commands/registry";
import { TOOLS } from "../tools/registry";
import en from "../locales/en.json";

const catalog = en as Record<string, string>;
const SRC = join(__dirname, "..");

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (/\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name)) out.push(p);
  }
  return out;
}

const files = walk(SRC).map((path) => ({
  path,
  rel: relative(SRC, path),
  sf: ts.createSourceFile(path, readFileSync(path, "utf8"), ts.ScriptTarget.ES2022, true, path.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS),
}));

const hasLetters = (s: string) => /\p{L}/u.test(s);
/** Attributes whose string value is shown to people or read by assistive tech. */
const TEXT_ATTRS = new Set(["aria-label", "aria-description", "aria-roledescription", "aria-placeholder", "aria-valuetext", "title", "alt", "placeholder", "label"]);

function visit(node: ts.Node, fn: (n: ts.Node) => void) {
  fn(node);
  ts.forEachChild(node, (c) => visit(c, fn));
}

describe("catalog", () => {
  it("English messages are valid ICU", () => {
    for (const [k, v] of Object.entries(catalog)) {
      expect(() => new IntlMessageFormat(v, "en"), k).not.toThrow();
    }
  });

  it("no hard-coded user-visible text in JSX", () => {
    const offenders: string[] = [];
    for (const { rel, sf } of files) {
      if (!rel.endsWith(".tsx")) continue;
      visit(sf, (n) => {
        const where = (node: ts.Node) => `${rel}:${sf.getLineAndCharacterOfPosition(node.getStart()).line + 1}`;
        if (ts.isJsxText(n) && hasLetters(n.text)) offenders.push(`${where(n)} text "${n.text.trim()}"`);
        if (ts.isJsxExpression(n) && n.expression && ts.isStringLiteralLike(n.expression) && hasLetters(n.expression.text) && !ts.isJsxAttribute(n.parent)) {
          offenders.push(`${where(n)} expression "${n.expression.text}"`);
        }
        if (ts.isJsxAttribute(n) && TEXT_ATTRS.has(n.name.getText())) {
          const init = n.initializer;
          const lit = init && ts.isStringLiteral(init) ? init : init && ts.isJsxExpression(init) && init.expression && ts.isStringLiteralLike(init.expression) ? init.expression : null;
          if (lit && hasLetters(lit.text)) offenders.push(`${where(n)} ${n.name.getText()}="${lit.text}"`);
        }
      });
    }
    expect(offenders).toEqual([]);
  });

  it("every literal catalog key used in code exists", () => {
    const prefixes = new Set(Object.keys(catalog).map((k) => k.split(".")[0] + "."));
    const missing: string[] = [];
    for (const { rel, sf } of files) {
      visit(sf, (n) => {
        if (!ts.isStringLiteralLike(n)) return;
        const text = n.text.replace(/^@/, "");
        if (!/^[a-z][A-Za-z0-9]*(\.[A-Za-z0-9_-]+)+$/.test(text)) return;
        if (!prefixes.has(text.split(".")[0] + ".")) return;
        // Only treat it as a key when it is passed to t() or lives in a *Key field / "@" param.
        const p = n.parent;
        const isT = ts.isCallExpression(p) && /^(t|i18next\.t)$/.test(p.expression.getText());
        const isKeyProp = ts.isPropertyAssignment(p) && /Key$/.test(p.name.getText());
        const isAt = n.text.startsWith("@");
        if ((isT || isKeyProp || isAt) && !(text in catalog)) missing.push(`${rel}: ${text}`);
      });
    }
    expect(missing).toEqual([]);
  });

  it("tools and commands only reference existing keys", () => {
    for (const tool of TOOLS) {
      expect(catalog[tool.titleKey], tool.id).toBeTypeOf("string");
      expect(catalog[tool.descriptionKey], tool.id).toBeTypeOf("string");
    }
    for (const c of COMMANDS) {
      expect(catalog[c.titleKey], c.id).toBeTypeOf("string");
      expect(catalog[c.categoryKey], c.id).toBeTypeOf("string");
    }
  });

  it("dynamic key families are complete", () => {
    for (const code of ["not-found", "cancelled", "encrypted", "corrupt", "io", "internal"]) expect(catalog[`error.${code}`], code).toBeTypeOf("string");
    for (const g of ["document", "edit", "protect", "convert", "forms", "compare", "print", "accessibility"]) expect(catalog[`group.${g}`], g).toBeTypeOf("string");
  });

  it("has no dead strings", () => {
    const literals = new Set<string>();
    for (const { sf } of files) visit(sf, (n) => ts.isStringLiteralLike(n) && literals.add(n.text.replace(/^@/, "")));
    const dynamic = ["error.", "group."];
    const dead = Object.keys(catalog).filter((k) => !literals.has(k) && !dynamic.some((d) => k.startsWith(d)));
    expect(dead).toEqual([]);
  });
});
