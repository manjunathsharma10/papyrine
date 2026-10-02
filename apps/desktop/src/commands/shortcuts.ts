export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

interface Parsed {
  mod: boolean;
  /** The literal Control key on every platform (tab switching). */
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  key: string;
}

export function parseShortcut(spec: string): Parsed {
  const parts = spec.split("+");
  const key = parts[parts.length - 1] as string;
  return { mod: parts.includes("Mod"), ctrl: parts.includes("Ctrl"), shift: parts.includes("Shift"), alt: parts.includes("Alt"), key };
}

function keyMatches(e: KeyboardEvent, key: string): boolean {
  if (key.length === 1) {
    const upper = key.toUpperCase();
    if (/[A-Z]/.test(upper)) return e.code === `Key${upper}`;
    if (/[0-9]/.test(key)) return e.code === `Digit${key}` || e.code === `Numpad${key}`;
    return e.key === key || (key === "=" && e.code === "Equal") || (key === "-" && e.code === "Minus");
  }
  return e.key === key;
}

/** True when the keyboard event is exactly this shortcut (Mod = Cmd on macOS, Ctrl elsewhere). */
export function matchesShortcut(e: KeyboardEvent, spec: string): boolean {
  const p = parseShortcut(spec);
  const wantCtrl = p.ctrl || (p.mod && !isMac);
  const wantMeta = p.mod && isMac;
  if (e.ctrlKey !== wantCtrl || e.metaKey !== wantMeta) return false;
  if (p.shift !== e.shiftKey || p.alt !== e.altKey) return false;
  return keyMatches(e, p.key);
}

const MAC_SYMBOLS: Record<string, string> = { Mod: "⌘", Shift: "⇧", Alt: "⌥", ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→", Home: "Home", End: "End" };
const WIN_NAMES: Record<string, string> = { Mod: "Ctrl", ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right" };

export function formatShortcut(spec: string): string {
  const parts = spec.split("+");
  if (isMac) return parts.map((p) => MAC_SYMBOLS[p] ?? p.toUpperCase()).join("");
  return parts.map((p) => WIN_NAMES[p] ?? p.toUpperCase()).join("+");
}

/** For aria-keyshortcuts. */
export function ariaShortcut(spec: string): string {
  return spec
    .split("+")
    .map((p) => (p === "Mod" ? (isMac ? "Meta" : "Control") : p === "Ctrl" ? "Control" : p))
    .join("+");
}

export function isEditableTarget(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false;
  return t.isContentEditable || t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || t instanceof HTMLSelectElement;
}
