# Papyrine marketing site

The public site for Papyrine, the open-source PDF editor in this repo. It is a static
[Astro](https://astro.build) project, deployed to GitHub Pages. Papyrine is pre-release, so the site
labels everything that isn't built yet as "Coming soon", marks every app mockup as a concept
illustration, and has no download buttons.

Plan and rationale: [PLAN.md](PLAN.md).

## Develop

Requires Node 22+.

```bash
cd site
npm install
npm run dev        # http://localhost:4321/papyrine/
```

The site is served under `/papyrine/` (the GitHub Pages project path). To serve from a root or
custom domain, set `SITE_BASE=/` (and `SITE_URL=https://example.com`) when building.

## Change the name, links or copy

Everything that can change lives in [`src/config.ts`](src/config.ts): the product name, tagline,
repo, licence string, the performance targets shown in the "Lightweight" scene, the compression
example numbers, and the roadmap milestones. Rename the product there. Then run `npm run og` to
regenerate `public/og.png`.

Scene copy is in `src/scenes/*.astro`; shared tokens (colours, fonts) are in `src/styles/tokens.css`.

## How the scroll animation works

Every animated element sits inside a `[data-tl]` timeline (a pinned section, or a scene's art).

- **Native:** CSS scroll-driven animations (`animation-timeline`, `view-timeline`) run each
  element's keyframes over a sub-range of the scene. No JavaScript is involved.
- **Fallback** (browsers without support, currently Firefox): `src/scripts/scroll-fallback.ts`
  writes scene progress to `--p` and the *same keyframes* are seeked with a negative
  `animation-delay`. It only tracks scenes near the viewport.
- **No JS, or `prefers-reduced-motion`:** neither mode turns on (`html.anim` is set by a tiny
  inline script only when motion is allowed). Every element's base style is its end state, so the
  page is a complete static illustration, and pinned scenes don't pin.

Only `transform` and `opacity` are animated, plus `stroke-dashoffset` on the pen and signature
strokes (the one approved exception). `npm run check` enforces this.

## Build and check

```bash
npm run build          # static output in dist/
npm run check          # animated properties, WCAG AA contrast (both themes), JS size budget
npm run preview        # serve dist/ at http://localhost:4321/papyrine/
SITE_URL=http://localhost:4321/papyrine/ npm run check:browser   # layout fits + fallback parity
npm run lhci           # Lighthouse CI (needs Chrome): 95+ in every category, LCP < 1.5 s
```

Budgets: under 100 KB of JavaScript gzipped (currently about 1 KB), Lighthouse 95+ in
performance, accessibility, best practices and SEO, LCP under 1.5 s on the mobile profile.

`npm run screenshots` captures every scene at 1440x900 and 390x844, in both themes, at the
start, middle and end of its scroll progress (`REDUCED=1` for the reduced-motion end states), and
`npm run contact-sheet` assembles them into `review-screenshots/`. Both use your installed Chrome.

## Deploy

[`.github/workflows/site.yml`](../.github/workflows/site.yml) builds and checks the site on every
change under `site/`. It deploys to GitHub Pages on pushes to `main`, but only when both are true:

1. Pages is enabled: Settings -> Pages -> Source: **GitHub Actions**.
2. The repository variable `PAGES_ENABLED` is `true`: Settings -> Secrets and variables ->
   Actions -> Variables.

Until then the workflow only verifies. The site would then be at
`https://manjunathsharma10.github.io/papyrine/`.

## Fonts

Self-hosted, subset to Latin, WOFF2 (about 48 KB total), under the SIL Open Font License 1.1:
[Instrument Serif](https://github.com/Instrument/instrument-serif) (display) and
[Geist](https://github.com/vercel/geist-font) (body). Source files come from the `@fontsource`
packages; `src/assets/fonts/` holds the subsets.
