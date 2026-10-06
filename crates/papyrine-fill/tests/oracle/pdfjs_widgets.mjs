// Test-only oracle: what pdf.js draws for each widget (the text of its appearance stream)
// and what it reads as the field value.
// usage: node pdfjs_widgets.mjs <pdfjs-dist dir> <file.pdf>
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import path from "node:path";

const [dir, file] = process.argv.slice(2);
const pdfjs = await import(pathToFileURL(path.join(dir, "node_modules/pdfjs-dist/legacy/build/pdf.mjs")).href);
const data = new Uint8Array(readFileSync(file));
const doc = await pdfjs.getDocument({ data, useSystemFonts: false, verbosity: 0, isEvalSupported: false }).promise;
const out = [];
for (let p = 1; p <= doc.numPages; p++) {
  const page = await doc.getPage(p);
  const annots = await page.getAnnotations();
  const meta = new Map();
  for (const a of annots) {
    if (a.subtype === "Widget" || a.subtype === "FreeText") meta.set(a.id, a);
  }
  const ops = await page.getOperatorList({ annotationMode: pdfjs.AnnotationMode.ENABLE });
  const O = pdfjs.OPS;
  let cur = null;
  const texts = new Map();
  for (let i = 0; i < ops.fnArray.length; i++) {
    const fn = ops.fnArray[i];
    const args = ops.argsArray[i];
    if (fn === O.beginAnnotation) {
      cur = args[0];
      texts.set(cur, "");
    } else if (fn === O.endAnnotation) {
      cur = null;
    } else if (cur !== null && fn === O.showText) {
      for (const g of args[0]) {
        if (g && typeof g === "object" && g.unicode) texts.set(cur, texts.get(cur) + g.unicode);
      }
    }
  }
  for (const [id, text] of texts) {
    const a = meta.get(id) ?? {};
    out.push({
      page: p - 1,
      id,
      subtype: a.subtype ?? null,
      fieldName: a.fieldName ?? null,
      fieldType: a.fieldType ?? null,
      fieldValue: a.fieldValue ?? null,
      checkBox: a.checkBox ?? false,
      radioButton: a.radioButton ?? false,
      buttonValue: a.buttonValue ?? null,
      exportValue: a.exportValue ?? null,
      hasAppearance: a.hasAppearance ?? null,
      rect: a.rect ?? null,
      text,
    });
  }
}
process.stdout.write(JSON.stringify(out));
