// Test-only pdf.js oracle: lists the annotations pdf.js parses from a PDF and samples pixels
// of the page rendered with annotations at 1 pt per pixel.
//   node check.mjs <file.pdf> <page-index> '<[[x,y],...]>' '<[[x0,y0,x1,y1],...]>'
// Points and regions are in PDF user space (origin bottom-left); regions report how many
// pixels are not white.
import fs from "node:fs";
import { createCanvas } from "@napi-rs/canvas";
import * as pdfjs from "pdfjs-dist/legacy/build/pdf.mjs";

const [file, pageArg = "0", pointsArg = "[]", regionsArg = "[]"] = process.argv.slice(2);
const data = new Uint8Array(fs.readFileSync(file));
const doc = await pdfjs.getDocument({
  data,
  useSystemFonts: false,
  isEvalSupported: false,
  verbosity: 0,
}).promise;
const page = await doc.getPage(Number(pageArg) + 1);
const annotations = (await page.getAnnotations()).map((a) => ({
  subtype: a.subtype,
  rect: Array.from(a.rect),
  contents: a.contentsObj?.str ?? null,
  title: a.titleObj?.str ?? null,
  color: a.color ? Array.from(a.color) : null,
  opacity: a.opacity ?? null,
  hasAppearance: !!a.hasAppearance,
  quadPoints: a.quadPoints ? Array.from(a.quadPoints, (q) => Object.values(q)) : null,
  inkLists: a.inkLists ?? null,
  lineCoordinates: a.lineCoordinates ?? null,
  borderWidth: a.borderStyle?.width ?? null,
  name: a.name ?? null,
}));
const viewport = page.getViewport({ scale: 1 });
const canvas = createCanvas(Math.ceil(viewport.width), Math.ceil(viewport.height));
const ctx = canvas.getContext("2d");
ctx.fillStyle = "white";
ctx.fillRect(0, 0, canvas.width, canvas.height);
await page.render({
  canvasContext: ctx,
  viewport,
  annotationMode: pdfjs.AnnotationMode.ENABLE,
}).promise;
const img = ctx.getImageData(0, 0, canvas.width, canvas.height);
const pixels = JSON.parse(pointsArg).map(([x, y]) => {
  const [px, py] = viewport.convertToViewportPoint(x, y);
  const i = (Math.floor(py) * canvas.width + Math.floor(px)) * 4;
  return [img.data[i], img.data[i + 1], img.data[i + 2]];
});
const regions = JSON.parse(regionsArg).map(([x0, y0, x1, y1]) => {
  const [ax, ay] = viewport.convertToViewportPoint(x0, y1);
  const [bx, by] = viewport.convertToViewportPoint(x1, y0);
  let n = 0;
  for (let y = Math.floor(ay); y < Math.ceil(by); y++) {
    for (let x = Math.floor(ax); x < Math.ceil(bx); x++) {
      const i = (y * canvas.width + x) * 4;
      if (img.data[i] < 250 || img.data[i + 1] < 250 || img.data[i + 2] < 250) n++;
    }
  }
  return n;
});
// Operator list: proves pdf.js really interpreted the appearance streams.
const ops = await page.getOperatorList({ annotationMode: pdfjs.AnnotationMode.ENABLE });
process.stdout.write(JSON.stringify({ pages: doc.numPages, annotations, pixels, regions, opCount: ops.fnArray.length }));
