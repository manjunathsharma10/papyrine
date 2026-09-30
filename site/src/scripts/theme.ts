// Three-way theme toggle: system -> light -> dark. `data-theme` is absent for "system".
const btn = document.querySelector<HTMLButtonElement>('[data-theme-toggle]');
const root = document.documentElement;
const order = ['system', 'light', 'dark'] as const;
type Mode = (typeof order)[number];
const names: Record<Mode, string> = { system: 'automatic', light: 'light', dark: 'dark' };

const current = (): Mode => {
  const t = root.getAttribute('data-theme');
  return t === 'light' || t === 'dark' ? t : 'system';
};
const paint = (m: Mode) => {
  if (!btn) return;
  btn.dataset.mode = m;
  btn.setAttribute('aria-label', `Colour theme: ${names[m]}. Activate to change.`);
};
if (btn) {
  btn.hidden = false;
  paint(current());
  btn.addEventListener('click', () => {
    const next = order[(order.indexOf(current()) + 1) % order.length];
    if (next === 'system') root.removeAttribute('data-theme');
    else root.setAttribute('data-theme', next);
    try {
      if (next === 'system') localStorage.removeItem('theme');
      else localStorage.setItem('theme', next);
    } catch {
      /* storage can be unavailable; the toggle still works for this visit */
    }
    paint(next);
  });
}
