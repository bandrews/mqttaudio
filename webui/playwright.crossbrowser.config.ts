import { defineConfig, devices } from '@playwright/test';

// Lane C cross-engine parity. Runs the same headless, fixture-backed e2e specs
// as Lane A (no real daemon), but across all three browser engine families —
// Chromium (Blink; also Edge), WebKit (Safari), and Firefox (Gecko) — to verify
// cross-browser rendering/interaction parity (MANUAL-VERIFICATION.md V-1). Not
// part of CI Lane A, which is chromium-only; run locally with
// `pnpm test:e2e:crossbrowser` (requires `playwright install webkit firefox`).
const PORT = 4174;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  reporter: 'list',
  use: {
    baseURL: `http://localhost:${PORT}`,
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    { name: 'webkit', use: { ...devices['Desktop Safari'] } },
    { name: 'firefox', use: { ...devices['Desktop Firefox'] } },
  ],
  webServer: {
    command: `pnpm exec vite --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
