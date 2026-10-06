import type { ChoiceOption, FieldKind, FormWidget, KeystrokeRequest, KeystrokeResult } from "./contract";

/**
 * Form model for MockHost: a fixed set of fields on every page of a mock
 * document that has `forms=1`, plus a tiny stand-in for the AF functions the
 * real engine implements in Rust (ADR-006). It exists so the UI can be
 * developed and tested without the engine; the formats here are not a
 * reference implementation.
 */

export type MockFormat = "none" | "number" | "percent" | "date" | "zip";

interface FieldTemplate {
  name: string;
  kind: FieldKind;
  rect: { x: number; y: number; width: number; height: number };
  format?: MockFormat;
  multiline?: boolean;
  password?: boolean;
  comb?: number;
  maxLength?: number;
  readOnly?: boolean;
  required?: boolean;
  editable?: boolean;
  multiSelect?: boolean;
  options?: ChoiceOption[];
  /** Radio group: one widget per entry. */
  radio?: { exportValue: string; rect: { x: number; y: number; width: number; height: number } }[];
  initial?: string;
  tooltip?: string;
  align?: "left" | "center" | "right";
}

const COUNTRIES: ChoiceOption[] = [
  { label: "Canada", value: "CA" },
  { label: "Germany", value: "DE" },
  { label: "India", value: "IN" },
];

const COLORS: ChoiceOption[] = [
  { label: "Red", value: "red" },
  { label: "Green", value: "green" },
  { label: "Blue", value: "blue" },
];

/** Positions sit between the page heading and the text lines, in points. */
const TEMPLATES: FieldTemplate[] = [
  { name: "fullName", kind: "text", rect: { x: 72, y: 130, width: 200, height: 20 }, required: true, tooltip: "Full name" },
  { name: "amount", kind: "text", rect: { x: 290, y: 130, width: 110, height: 20 }, format: "number", align: "right", tooltip: "Amount" },
  { name: "zip", kind: "text", rect: { x: 410, y: 130, width: 90, height: 20 }, format: "zip", maxLength: 5, tooltip: "ZIP code" },
  { name: "code", kind: "text", rect: { x: 72, y: 160, width: 150, height: 20 }, comb: 6, maxLength: 6, tooltip: "Reference code" },
  { name: "secret", kind: "text", rect: { x: 240, y: 160, width: 120, height: 20 }, password: true, tooltip: "Passphrase" },
  { name: "locked", kind: "text", rect: { x: 380, y: 160, width: 120, height: 20 }, readOnly: true, initial: "Read only", tooltip: "Locked field" },
  { name: "notes", kind: "text", rect: { x: 72, y: 190, width: 428, height: 52 }, multiline: true, tooltip: "Notes" },
  { name: "agree", kind: "checkbox", rect: { x: 72, y: 252, width: 16, height: 16 }, tooltip: "I agree" },
  {
    name: "choice",
    kind: "radio",
    rect: { x: 120, y: 252, width: 16, height: 16 },
    radio: [
      { exportValue: "A", rect: { x: 120, y: 252, width: 16, height: 16 } },
      { exportValue: "B", rect: { x: 150, y: 252, width: 16, height: 16 } },
      { exportValue: "C", rect: { x: 180, y: 252, width: 16, height: 16 } },
    ],
    tooltip: "Choice",
  },
  { name: "country", kind: "combo", rect: { x: 220, y: 248, width: 120, height: 22 }, options: COUNTRIES, tooltip: "Country" },
  { name: "colors", kind: "list", rect: { x: 360, y: 248, width: 100, height: 46 }, options: COLORS, tooltip: "Colour" },
];

export interface MockWidgetDef {
  id: string;
  fieldBase: string;
  template: FieldTemplate;
  rect: FieldTemplate["rect"];
  exportValue?: string;
}

/** Widget definitions for a page, independent of values. */
export function widgetDefs(pageUid: number): MockWidgetDef[] {
  const out: MockWidgetDef[] = [];
  for (const t of TEMPLATES) {
    if (t.radio) {
      t.radio.forEach((r) => out.push({ id: `w${pageUid}:${t.name}:${r.exportValue}`, fieldBase: t.name, template: t, rect: r.rect, exportValue: r.exportValue }));
    } else {
      out.push({ id: `w${pageUid}:${t.name}`, fieldBase: t.name, template: t, rect: t.rect, exportValue: t.kind === "checkbox" ? "Yes" : undefined });
    }
  }
  return out;
}

export function initialValues(fieldSuffix: string): Record<string, string> {
  const v: Record<string, string> = {};
  for (const t of TEMPLATES) {
    if (t.kind === "button" || t.kind === "signature") continue;
    v[t.name + fieldSuffix] = t.initial ?? (t.kind === "checkbox" ? "false" : "");
  }
  return v;
}

export function fieldIdOf(base: string, suffix: string): string {
  return base + suffix;
}

/** Digits-only keystroke filter for the number and ZIP formats. */
export function mockKeystroke(format: MockFormat, req: KeystrokeRequest): KeystrokeResult {
  const next = req.value.slice(0, req.selStart) + req.change + req.value.slice(req.selEnd);
  if (format === "number" || format === "percent") {
    if (req.commit) {
      const raw = next.replace(/[$,%\s]/g, "");
      return raw === "" || !Number.isNaN(Number(raw)) ? { accept: true, value: next } : { accept: false, value: req.value, message: "Enter a number" };
    }
    return /^[0-9.,$%\-\s]*$/.test(req.change) ? { accept: true, value: next } : { accept: false, value: req.value, message: "Only digits are accepted" };
  }
  if (format === "zip") {
    if (req.commit) return next === "" || /^\d{5}$/.test(next) ? { accept: true, value: next } : { accept: false, value: req.value, message: "Enter a 5-digit ZIP code" };
    return /^\d*$/.test(req.change) ? { accept: true, value: next } : { accept: false, value: req.value, message: "Only digits are accepted" };
  }
  return { accept: true, value: next };
}

/** What the field shows when it is not being edited. */
export function mockFormat(format: MockFormat, value: string): string {
  if (value === "") return "";
  if (format === "number") {
    const n = Number(value.replace(/[$,\s]/g, ""));
    return Number.isNaN(n) ? value : `$${n.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  }
  if (format === "percent") {
    const n = Number(value.replace(/[%\s]/g, ""));
    return Number.isNaN(n) ? value : `${n.toFixed(1)}%`;
  }
  return value;
}

export function templateOf(base: string): FieldTemplate | undefined {
  return TEMPLATES.find((t) => t.name === base);
}

/** Build the wire widgets for one page. `tabStart` is the global tab index of its first widget. */
export function buildWidgets(page: number, pageUid: number, suffix: string, values: Record<string, string>, tabStart: number): FormWidget[] {
  return widgetDefs(pageUid).map((d, i) => {
    const t = d.template;
    const fieldId = fieldIdOf(d.fieldBase, suffix);
    const value = values[fieldId] ?? "";
    return {
      id: d.id,
      fieldId,
      name: fieldId,
      page,
      rect: d.rect,
      kind: t.kind,
      value,
      display: t.kind === "text" ? mockFormat(t.format ?? "none", value) : value,
      exportValue: d.exportValue,
      options: t.options,
      multiline: !!t.multiline,
      password: !!t.password,
      comb: t.comb ?? 0,
      maxLength: t.maxLength ?? 0,
      readOnly: !!t.readOnly,
      required: !!t.required,
      editable: !!t.editable,
      multiSelect: !!t.multiSelect,
      fontSize: 0,
      align: t.align ?? "left",
      tabIndex: tabStart + i,
      tooltip: t.tooltip ?? "",
    };
  });
}
