import { defineConfig, devices } from "@playwright/test";

const PORT = 1431;

export default defineConfig({
  testDir: ".",
  testMatch: /.*\.spec\.ts/,
  outputDir: "./artifacts/test-results",
  fullyParallel: true,
  workers: 4,
  retries: 0,
  reporter: [["list"]],
  timeout: 30_000,
  expect: { timeout: 7_000 },
  use: {
    baseURL: `http://localhost:${PORT}`,
    ...devices["Desktop Chrome"],
    viewport: { width: 1280, height: 800 },
    deviceScaleFactor: 1,
    trace: "off",
  },
  webServer: {
    // Own port so a developer's `vite` on 1420 is never reused or clobbered.
    command: `pnpm exec vite --port ${PORT} --strictPort`,
    cwd: "..",
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
