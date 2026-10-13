import { defineConfig, devices } from "@playwright/test";

/**
 * Mocked-IPC UI tests: Vite serves the real React app, the Tauri bridge and the
 * CodeWhale runtime are faked in-process (see mock/). No Rust, no CodeWhale, no Ollama.
 *
 * Env:
 *   E2E_PORT=1420           Vite port (an already running dev server on it is reused locally)
 *   CW_E2E_CHROMIUM=path    use this Chromium binary instead of Playwright's
 *   CW_E2E_FIXTURES=synthetic  ignore fixtures/live and use only synthetic fixtures
 */
const PORT = Number(process.env.E2E_PORT ?? 1420);
const CI = !!process.env.CI;

export default defineConfig({
  testDir: "./tests",
  timeout: 30_000,
  expect: { timeout: 8_000 },
  fullyParallel: true,
  forbidOnly: CI,
  retries: CI ? 1 : 0,
  workers: CI ? 2 : undefined,
  reporter: CI ? [["list"], ["html", { open: "never" }]] : [["list"], ["html", { open: "never" }]],
  outputDir: "test-results",
  use: {
    ...devices["Desktop Chrome"],
    baseURL: `http://localhost:${PORT}`,
    viewport: { width: 1280, height: 800 },
    locale: "ru-RU",
    timezoneId: "Europe/Moscow",
    colorScheme: "dark",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    launchOptions: process.env.CW_E2E_CHROMIUM ? { executablePath: process.env.CW_E2E_CHROMIUM } : {},
  },
  projects: [{ name: "chromium" }],
  webServer: {
    // The app's own Vite config (React + Tailwind), started from the project root.
    command: `npx vite --port ${PORT} --strictPort`,
    cwd: "..",
    url: `http://localhost:${PORT}`,
    reuseExistingServer: !CI,
    timeout: 120_000,
    stdout: "ignore",
    stderr: "pipe",
  },
});
