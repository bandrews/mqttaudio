import { defineConfig, devices } from '@playwright/test';

// Lane B (real browser + live daemon). Runs the SPA through the Vite dev proxy
// (the sidecar mirror, DW1/F2) against a REAL mqttaudio daemon that the spec
// spawns/kills. Not part of CI Lane A (needs the native daemon + an audio
// device); run locally with `pnpm test:e2e:laneb`.
const PORT = 4273;
const DAEMON_PORT = 8099;

export default defineConfig({
  testDir: './e2e-lane-b',
  fullyParallel: false,
  workers: 1,
  reporter: 'list',
  timeout: 90_000,
  use: {
    baseURL: `http://localhost:${PORT}`,
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: `node node_modules/vite/bin/vite.js --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    timeout: 120_000,
    env: { VITE_DAEMON_TARGET: `http://127.0.0.1:${DAEMON_PORT}` },
  },
});
