# Host end-to-end tests (dev bridge)

Headless Chromium drives the **real UI** against the **real engine and renderer** through
the dev bridge (`papyrine-bridge`, cargo feature `dev-bridge`). No window of the app is
ever opened.

```sh
cargo build -p papyrine-desktop --features dev-bridge --bin papyrine-bridge
cd apps/desktop && pnpm exec playwright test -c src-tauri/e2e/playwright.config.ts
```

`PAPYRINE_BRIDGE_BIN` points at a prebuilt bridge binary (otherwise `target/*/debug` is searched).
The Vite dev server runs on its own port (1432). Each test starts its own bridge process on a
temporary data directory; the crash test kills it with SIGKILL and starts a second one on the
same directory.
