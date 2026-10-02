# Desktop UI tests

Everything here runs against the Vite dev server with the in-memory `MockHost`
(`src/ipc/mock.ts`); no Tauri window is ever opened.

```sh
pnpm test            # vitest: fuzzy matcher, zoom math, tile cache, i18n + catalog guards
pnpm e2e             # Playwright (headless Chromium, own port 1431): keyboard flows, axe, i18n/RTL
pnpm build && pnpm bundle:report   # gzip sizes, fails if the initial load exceeds 200 KB
```

Screenshots and Playwright output go to `e2e/artifacts/` (gitignored). The specs:

- `keyboard.spec.ts`, `viewer.spec.ts`, `tools.spec.ts`, `palette-theme.spec.ts`: keyboard-only flows.
- `a11y.spec.ts`: axe-core (serious/critical must be zero) in light, dark, high-contrast, forced-colors and RTL.
- `i18n.spec.ts`: pseudo-locale (en-XA) and RTL (ar-XB) layout checks plus screenshots.
- `perf.spec.ts`: indicative scroll frame times with the mock host (B4 is measured on the real host).

Hard-coded UI text is caught by `src/i18n/catalog.test.ts`, which parses every `.tsx` file.
