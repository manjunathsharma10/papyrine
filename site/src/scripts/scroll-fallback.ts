// Fallback for browsers without CSS scroll-driven animations (html.sfb, set in <head>).
// Writes --p (scene progress, 0..1) on each [data-tl] element; motion.css seeks the same
// keyframes with it. The maths mirrors the native ranges: `contain` for pinned scenes, and
// a `cover` window (--rs/--rw, read from CSS) for reveal scenes. Only scenes near the viewport
// are tracked.
const root = document.documentElement;

if (root.classList.contains('sfb')) {
  const scenes = Array.from(document.querySelectorAll<HTMLElement>('[data-tl]'));
  const near = new Set<HTMLElement>();
  let queued = false;

  const clamp = (n: number) => Math.min(1, Math.max(0, n));

  const progress = (el: HTMLElement, vh: number): number => {
    const r = el.getBoundingClientRect();
    if (el.dataset.mode === 'pin') {
      const travel = r.height - vh;
      return travel > 0 ? clamp(-r.top / travel) : 0;
    }
    const cs = getComputedStyle(el);
    const rs = parseFloat(cs.getPropertyValue('--rs'));
    const rw = parseFloat(cs.getPropertyValue('--rw'));
    const cover = ((vh - r.top) / (vh + r.height)) * 100; // 0 as it enters, 100 as it leaves
    return clamp((cover - rs) / rw);
  };

  const update = () => {
    queued = false;
    const vh = window.innerHeight;
    for (const el of near) el.style.setProperty('--p', progress(el, vh).toFixed(4));
  };
  const schedule = () => {
    if (!queued) {
      queued = true;
      requestAnimationFrame(update);
    }
  };

  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        const el = e.target as HTMLElement;
        if (e.isIntersecting) near.add(el);
        else {
          // One last write so a fast scroll past a scene leaves it at its true end state.
          el.style.setProperty('--p', progress(el, window.innerHeight).toFixed(4));
          near.delete(el);
        }
      }
      schedule();
    },
    { rootMargin: '50% 0px' },
  );
  scenes.forEach((s) => io.observe(s));
  addEventListener('scroll', schedule, { passive: true });
  addEventListener('resize', schedule, { passive: true });
}
