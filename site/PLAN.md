# Folio marketing site — plan

Status: **approved and built** (see README.md).

> **Rename:** this plan was written when the working name was Folio. The owner has since accepted
> **Papyrine** (ADR-019) and the repo is now `papyrine`. The site reads the name and repo from
> `src/config.ts`, so it now says Papyrine and is served under `/papyrine/`. Where this plan says
> "Folio", read the product name.

## 0. Constraints from the repo

- Folio is in **planning**. The README says "no application code yet", and
  the ROADMAP is Draft v2, awaiting approval. There is no build, so **every
  feature is "Coming soon"**, including v0.1 ones.
- Every app-window or UI mockup carries a visible **"Concept illustration"**
  caption. None of them is a screenshot.
- The hero CTA is **"Follow development on GitHub"** → the repo. There are no
  download buttons.
- Numbers on the site come only from the ARCHITECTURE budgets or are labelled
  "example". They are always labelled "target" or "example", never "measured".
  - Installer: ≤ 50 MB hard ceiling, ≤ 30 MB internal goal.
  - Cold launch to first page: < 1.0 s.
- The product name, repo URL, licence string and per-feature status live in
  **one file**: `site/src/config.ts`. Every page reads from it, so a rename is
  a one-line change. The per-feature status comes from the ROADMAP milestone
  (v0.1 … v0.3).

## 1. Page structure

| # | Section | Behaviour | Status label |
|---|---|---|---|
| – | Header: wordmark, anchor nav, GitHub link, theme toggle | sticky, static | – |
| 1 | **Hero**: "Everything you need from a PDF editor, without the bloat." | **pinned**, about 250svh | – |
| – | At a glance: fast launch, small install, offline, private, cross-platform, free and open source | static row | budgets marked "target" |
| 2 | **Annotate**: highlights, comment bubble, pen stroke | reveal-driven | Coming soon |
| 3 | **Organise pages**: reorder, rotate, merge two documents | **pinned**, about 300svh | Coming soon |
| 4 | **Fill and sign** | reveal-driven | Coming soon (signing is v0.3) |
| 5 | **Compress** | **pinned**, about 250svh | Coming soon (v0.2) |
| 6 | **Lightweight**: install size and launch bars | reveal-driven | "Target", not measured |
| 7 | **Private and offline** | reveal-driven | – |
| 8 | **Open source**: repo, licence, Contribute | reveal | – |
| – | Footer: licence, repo, "pre-release" note | static | – |

## 2. Animation concepts

The mechanics are shared across scenes:

- **Progress is a pure function of scroll position.** Jumping via an anchor
  link or a fast fling always lands on a coherent frame.
- **Every scene's message is in real HTML text** (heading, caption, status
  chip), outside the animated area. The animation shows the feature and
  nothing depends on it to be understood.
- **Start and end states are both meaningful.** The anchor target is the
  scene's top, with the heading visible.
- **Pinned scenes** use `position: sticky` inside a tall section, with `svh`
  units. There is no scroll hijacking.
- **Mobile is a separate layout, not a scaled desktop.**
  - Pinned scenes are shorter (about 180–220svh).
  - Layouts are vertical.
  - Choreography is simplified where the desktop version doesn't fit.

| Scene | Desktop | Mobile |
|---|---|---|
| **1 Hero** | The page floats in on load (CSS only, so the LCP element is the H1 text). While pinned: one page fans into 5 (rotate and translate), then the pages converge into the window's page grid. The window chrome fades in, and the caption reads "Concept illustration". | Same story, narrower fan, window shown portrait. |
| **2 Annotate** | Over one page: 3 highlights wipe on (`scaleX`), a comment bubble pops in, and a freehand pen stroke draws itself along its path. | Same page, larger relative to the viewport, staggered vertically. |
| **3 Organise** | Two documents, A (4 pages) and B (3 pages). Page slots are defined with custom properties `--x0/--y0 → --x1/--y1`, and pages only `translate`/`rotate` between them. Phases: reorder in A, rotate one page 90°, then interleave A and B into one row. The counter reads "7 pages · 1 file". | Slot coordinates are redefined per breakpoint, so pages travel in a compact grid instead of two rows. |
| **4 Fill and sign** | The form is a static SVG. Text is "typed" by a paper-coloured cover sliding away (`transform`). The checkbox tick draws. The signature writes itself along its path. | Fields stacked, larger. |
| **5 Compress** | Before/after split, with the divider dragged by scroll (`translateX` on a clipped layer) from "Original 24 MB" to "Balanced 3.1 MB". The counter is a small tabular-nums figure. Both figures carry an "Example numbers" label. | The split is horizontal, with the same handle. |
| **6 Lightweight** | Two rows of bars grow (`scaleX`), each with "Folio target" against a reference bar. Installer and launch time are labelled "target — benchmarks pending". | Bars stack. |
| **7 Offline** | A laptop with a document on screen. Network lines (to cloud icons) go dashed, then split and fade, and an "Offline" badge appears. A highlight keeps drawing on the document afterwards, showing the editor still works. | Same, laptop scaled. |
| **8 Open source** | Repo card, licence chip "MIT OR Apache-2.0", Contribute → GitHub. It reveals as a simple fade and rise. | Same. |

**Paper motif** (subtle, not clip-art):

- Warm paper background in light mode. In dark mode the page is ink-dark but
  the *pages* stay paper-coloured.
- Section markers use folio-style page numbers in the margin. Ruled margin
  lines. One dog-eared corner (clip-path) on the hero page.
- Grain is a tiny tiled SVG, not a filter.

**Two known conflicts with "animate only transform and opacity"** (see §7):

- Pen and signature strokes use `stroke-dashoffset`.
- The size counter changes its text.

Both are limited to tiny SVG or text nodes. I'll confirm with a performance
trace that they don't cost frames.

## 3. Tech choices

- **Astro, static output.** It ships zero client JS by default. Scenes are
  Astro components with inline SVG. It also handles fonts and images at build
  time, and gives me `base` handling for GitHub Pages.
- **Native CSS scroll-driven animations** (`animation-timeline: view()`) under
  `@supports`. Where unsupported (currently Firefox), a ~2 KB script:
  - uses `IntersectionObserver` to activate only near-viewport scenes;
  - writes progress in a `requestAnimationFrame`-throttled scroll handler;
  - drives the **same keyframes** by setting a negative `animation-delay`
    with `animation-play-state: paused`. There is one set of keyframes, not two.
- **No GSAP, no Lenis.**
  - GSAP with ScrollTrigger would be about 35 KB gzipped, and nothing here
    needs it: all scenes are linear scroll-to-progress mappings.
  - Lenis replaces native scrolling, which the brief rules out.
  - Expected total JS: **under 10 KB gzipped** (budget 100 KB). It covers the
    fallback, the theme toggle and the scene lazy-init.
- **Theme:** `prefers-color-scheme` by default, with a system/light/dark
  toggle. A tiny inline head script prevents a flash. `localStorage` reads
  and writes are wrapped in try/catch.
- **Fonts** (OFL, self-hosted, subset to Latin, WOFF2, preloaded):
  - Display: **Instrument Serif** (regular and italic).
  - Body: **Geist**, variable.
  - Target under 50 KB total. Final choice confirmed against a specimen in
    the first review round.
- **Accent:** one "proof-mark" vermilion. Text-use variants are darkened so
  they pass AA on paper and on ink. A build-time script checks every
  foreground/background token pair in both themes.
- **Images:** inline SVG only. The one raster is the OG image (PNG, required
  by social platforms).
- **Lazy scene init:** scenes only run near the viewport
  (`IntersectionObserver` with a `rootMargin`, plus `content-visibility:
  auto`).

## 4. Accessibility and resilience

- **Reduced motion:** all scroll animations are disabled. Each scene shows its
  static end-state SVG, and simple fades remain for reveals.
- **No JavaScript:** the page is fully readable. Animations are wrapped in
  `@media (prefers-reduced-motion: no-preference)` plus `@supports`, or
  enabled by a `js` class. Without either, users see the static end state.
- Semantic landmarks and one `h1`. Visible focus rings. Skip link.
- Every visual is an inline SVG with `<title>` and alt text, or `aria-hidden`
  when it is decorative and its meaning is already in the caption.
- WCAG AA contrast is checked in both themes by the token script and by axe.

## 5. Performance budgets and CI

| Budget | Gate |
|---|---|
| JS ≤ 100 KB gzipped (target under 10 KB) | script over `dist/**/*.js` in CI |
| Lighthouse ≥ 95 for performance, accessibility, best practices and SEO | **Lighthouse CI**, mobile preset, on PRs and on main |
| LCP < 1.5 s | LHCI assertion (see the caveat below) |
| First-load transfer: about 150 KB (HTML, CSS, JS, fonts) | reported in CI |

**LCP caveat:** Lighthouse's default mobile profile is Slow 4G with 4× CPU
throttling, so 1.5 s LCP is tight. I'll get there with inline critical CSS,
a preloaded subset font, no render-blocking assets, and a text LCP. I'll
report actual numbers, and if a budget turns out unreachable I'll tell you
rather than loosen it quietly.

**Animation guardrails:** every keyframe touches `transform` or `opacity`
only, apart from the two exceptions in §2, enforced by a stylelint rule on
the animation files.

## 6. Deliverables

```
site/
  PLAN.md  README.md  package.json  astro.config.mjs  lighthouserc.json
  src/config.ts             name, repo, licence, feature statuses
  src/scenes/*.astro        one component per scene, own CSS
  src/styles/               tokens, base, animations
  src/scripts/scroll-fallback.ts   theme.ts
  public/fonts/  public/og.png
  scripts/                  contrast check, JS-size gate, screenshots
.github/workflows/site.yml  build + LHCI always; deploy only on main
```

- **Deploy:** GitHub Pages via the official `configure-pages`,
  `upload-pages-artifact` and `deploy-pages` actions. The deploy job runs only
  when the repo variable `PAGES_ENABLED == 'true'`. **I won't enable Pages or
  set that variable**, so pushes to main stay green until you confirm.
- **`base` path:** `/folio/` (from `github.com/manjunathsharma10/folio`,
  served at `manjunathsharma10.github.io/folio/`). It comes from `config.ts`,
  so a custom domain only needs `base: '/'`.
- **README:** dev, build, deploy, and how to edit the config.
- **Screenshots:** a Playwright script captures every section at
  **1440×900 and 390×844, light and dark**. Pinned scenes get start, middle
  and end frames. Output goes to `site/review-screenshots/` (gitignored) and I
  will show you a contact sheet. The same script runs axe-core.
- **Repo hygiene:** commit and push at the end of each task, per your standing
  instruction. I won't touch your uncommitted `docs/` edits.

## 7. Decisions I need from you

1. **Reference bars in scene 6.** I have no measured competitor data and won't
   invent it. My default is **Folio's target vs a generic "typical bundled
   suite (illustrative)" bar**, with no named products. Alternative: show
   only Folio's target against its own 50 MB ceiling.
2. **The `stroke-dashoffset` exception** for pen and signature strokes and the
   text counter. My default is to allow it on those tiny elements. The
   alternative is to fake the draw with a sweeping mask, which looks worse on
   freehand curves.
3. **"Free forever."** MIT OR Apache-2.0 guarantees the *code* stays free, but
   "forever" is a promise about the project. My default is "**Free and open
   source, always**" in the headline. Please confirm or change it.
4. **Accent and fonts:** vermilion accent with Instrument Serif and Geist. The
   alternative accent is ink-indigo. I'll show both in the first screenshot
   round if you want to compare.
5. **"Works fully offline."** ARCHITECTURE v0.5 adds optional downloadable
   components (OCR languages, etc.). My copy will say "works fully offline"
   for the core app and won't mention components. I'll mention them only if
   you want that transparency.

## 8. Build order (after approval)

1. Scaffold, tokens, themes, fonts, all copy and static SVG end-states. This
   is the complete no-JS page. Also CI skeleton and the deploy workflow.
2. Scenes 1, 3 and 5, the pinned ones, plus the scroll fallback.
3. Scenes 2, 4, 6, 7 and 8.
4. Performance, accessibility and reduced-motion passes, Lighthouse CI
   tuned, README.
5. Screenshot pass and contact sheet for your review.
