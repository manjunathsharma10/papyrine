/**
 * Radix returns focus to a Dialog.Trigger on close; ours open from shortcuts and
 * the palette, so there is none. Track the last real focus target and put focus
 * back there unless a command already moved it somewhere deliberate.
 */
let last: HTMLElement | null = null;

export function trackFocus(): void {
  document.addEventListener(
    "focusin",
    (e) => {
      const t = e.target;
      if (t instanceof HTMLElement && !t.closest('[role="dialog"], [role="menu"], [data-radix-popper-content-wrapper]')) last = t;
    },
    true,
  );
}

export function restoreFocus(e?: Event): void {
  e?.preventDefault();
  const a = document.activeElement;
  if (a && a !== document.body && a.isConnected && !a.closest('[role="dialog"]')) return;
  const target = last?.isConnected ? last : document.querySelector<HTMLElement>('[data-region="viewer"]');
  target?.focus({ preventScroll: true });
}
