// ABOUTME: Playwright config for Lane A: headless Chromium runs of the e2e/ specs.
// ABOUTME: Serves the app with Vite on port 4173; the specs mock the daemon with route handlers.

import { defineConfig, devices } from '@playwright/test';

// Headless E2E (Lane A). The app is served by Vite against a route-mocked
// (fixture) backend inside the spec — no real daemon is contacted in CI.
const PORT = 4173;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? 'line' : 'list',
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: 'on-first-retry',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: `node node_modules/vite/bin/vite.js --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
