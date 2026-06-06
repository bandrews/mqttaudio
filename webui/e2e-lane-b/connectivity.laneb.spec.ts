import { test, expect } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

// Lane B: drive the SPA through the dev proxy against a REAL daemon (spawned by
// this spec). Verifies connectivity + restart survival and that the dashboard
// reflects a real play/stop within the poll interval.
// NOTE: real /ws log LINES are gated on a daemon gap (WebSocketLogLayer not wired;
// see docs/bugs.md, Sprint W1) — the connectivity claims here use the welcome
// frame, the live state, and the backoff reconnect.

const BIN = resolve(process.cwd(), '..', 'target', 'release', 'mqttaudio');
const WAV = resolve(process.cwd(), '..', 'tests', 'audio', 'test_beep_5s.wav');
const DAEMON_PORT = 8099;
const HEALTH = `http://127.0.0.1:${DAEMON_PORT}/health`;

async function startDaemon(): Promise<ChildProcess> {
  const proc = spawn(BIN, ['--http-port', String(DAEMON_PORT)], { stdio: 'ignore' });
  for (let i = 0; i < 100; i++) {
    try {
      const res = await fetch(HEALTH);
      if (res.ok) return proc;
    } catch {
      /* not up yet */
    }
    await sleep(100);
  }
  proc.kill('SIGKILL');
  throw new Error('daemon did not become healthy');
}

async function waitDaemonDown(): Promise<void> {
  for (let i = 0; i < 50; i++) {
    try {
      await fetch(HEALTH);
    } catch {
      return;
    }
    await sleep(100);
  }
}

test.describe.configure({ mode: 'serial' });

let daemon: ChildProcess | undefined;

test.afterEach(() => {
  daemon?.kill('SIGKILL');
  daemon = undefined;
});

test('connects to a real daemon through the proxy and survives a restart', async ({ page }) => {
  daemon = await startDaemon();

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();

  await expect(page.getByText('Log stream')).toBeVisible();
  await expect(page.getByText('daemon v2.0.0')).toBeVisible({ timeout: 20_000 });

  // Kill the daemon: socket closes, the /version re-probe fails -> offline.
  daemon.kill('SIGKILL');
  await waitDaemonDown();
  await expect(page.getByText('offline — reconnecting')).toBeVisible({ timeout: 20_000 });

  // Bring it back: backoff reconnect re-subscribes and marks the gap.
  daemon = await startDaemon();
  await expect(page.getByText(/messages during the gap were dropped/i).first()).toBeVisible({
    timeout: 45_000,
  });
});

test('dashboard reflects a real play and stop within the poll interval', async ({ page }) => {
  daemon = await startDaemon();

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();
  await expect(page.getByText(/no samples playing/i)).toBeVisible({ timeout: 20_000 });

  // Play a looping file through the proxy; the now-playing board polls /status/samples.
  await page.request.post('/api/command', {
    data: { command: 'play', message: { file: WAV, voice: 'beep', loop: true } },
  });
  await expect(page.getByText('test_beep_5s.wav')).toBeVisible({ timeout: 6_000 });
  await expect(page.getByText('voice: beep')).toBeVisible();

  // Stop everything; the board returns to empty.
  await page.request.post('/api/command', { data: { command: 'stopall' } });
  await expect(page.getByText(/no samples playing/i)).toBeVisible({ timeout: 6_000 });
});

test('the console cue launcher plays a real file and Stop All clears it', async ({ page }) => {
  daemon = await startDaemon();

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();
  await expect(page.getByText('Log stream')).toBeVisible({ timeout: 20_000 });

  // Drive the cue launcher in the Console tab.
  await page.getByRole('tab', { name: 'Console' }).click();
  await page.getByLabel('file', { exact: true }).first().fill(WAV);
  await page.getByLabel('loop').check();
  await page.getByRole('button', { name: 'Play' }).click();

  // The Monitor tab reflects the real play.
  await page.getByRole('tab', { name: 'Monitor' }).click();
  await expect(page.getByText('test_beep_5s.wav')).toBeVisible({ timeout: 6_000 });

  // Stop All from the console clears it.
  await page.getByRole('tab', { name: 'Console' }).click();
  await page.getByRole('button', { name: 'Stop all' }).click();
  await page.getByRole('tab', { name: 'Monitor' }).click();
  await expect(page.getByText(/no samples playing/i)).toBeVisible({ timeout: 6_000 });
});

test('the matrix mixer routes a real play (dest 0/1 on the default device)', async ({ page }) => {
  daemon = await startDaemon();

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();
  await expect(page.getByText('Log stream')).toBeVisible({ timeout: 20_000 });

  await page.getByRole('tab', { name: 'Matrix' }).click();
  await page.getByLabel('matrix file').fill(WAV);
  // Route to the first two output channels (valid on any >= 2ch device; routing
  // to higher channels needs multichannel hardware — out-of-range routes are
  // silently skipped by the daemon).
  await page.getByLabel('route 0 to 0').check();
  await page.getByLabel('route 1 to 1').check();
  await page.getByRole('button', { name: 'Play routed' }).click();

  await page.getByRole('tab', { name: 'Monitor' }).click();
  await expect(page.getByText('test_beep_5s.wav')).toBeVisible({ timeout: 6_000 });

  await page.request.post('/api/command', { data: { command: 'stopall' } });
});

test('the mixer transport shows a real sample and its Stop control clears it', async ({ page }) => {
  daemon = await startDaemon();

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();
  await expect(page.getByText('Log stream')).toBeVisible({ timeout: 20_000 });

  await page.request.post('/api/command', {
    data: { command: 'play', message: { file: WAV, voice: 'beep', loop: true } },
  });

  // The mixer transport reflects the real sample with seek + speed controls.
  await page.getByRole('tab', { name: 'Mixer' }).click();
  await expect(page.getByText('test_beep_5s.wav')).toBeVisible({ timeout: 6_000 });
  await expect(page.getByLabel(/^seek /)).toBeVisible();

  // Stop it from the transport card; the monitor clears.
  await page.getByRole('button', { name: 'Stop' }).first().click();
  await page.getByRole('tab', { name: 'Monitor' }).click();
  await expect(page.getByText(/no samples playing/i)).toBeVisible({ timeout: 6_000 });
});

test('the config tab shows the real running config and emits a snippet (Sprint W8)', async ({ page }) => {
  daemon = await startDaemon();

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();
  await expect(page.getByText('Log stream')).toBeVisible({ timeout: 20_000 });

  await page.getByRole('tab', { name: 'Config' }).click();
  // GET /config round-trips through the proxy to the real daemon and is shown.
  await expect(page.getByLabel('running config')).toContainText('audio', { timeout: 8_000 });
  // The output-stage editor produces a restart-required snippet.
  await expect(page.getByLabel('audio output stage snippet')).toContainText('master_gain');
});

test('telemetry on: output meters move with signal over /ws/state (Sprint W7)', async ({ page }) => {
  daemon = await startDaemon();

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();
  await expect(page.getByText('Log stream')).toBeVisible({ timeout: 20_000 });

  await page.getByRole('checkbox', { name: 'telemetry' }).click();
  await page.request.post('/api/command', {
    data: { command: 'play', message: { file: WAV, voice: 'beep', loop: true } },
  });

  await page.getByRole('tab', { name: 'Monitor' }).click();
  // The output meter (driven by the /ws/state tick channel) moves past 0 with the
  // tone playing — the W7 state channel + meters working end-to-end.
  const meter = page.getByLabel('output meter 0');
  await expect(meter).toBeVisible({ timeout: 8_000 });
  await expect
    .poll(async () => Number(await meter.getAttribute('aria-valuenow')), { timeout: 8_000 })
    .toBeGreaterThan(0);

  await page.request.post('/api/command', { data: { command: 'stopall' } });
});

test('telemetry on: live progress advances against a real daemon (Sprint W6)', async ({ page }) => {
  daemon = await startDaemon();

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();
  await expect(page.getByText('Log stream')).toBeVisible({ timeout: 20_000 });

  // Opt in to telemetry, then play a looping file. (The switch is controlled by
  // the /telemetry query, so click rather than check.)
  await page.getByRole('checkbox', { name: 'telemetry' }).click();
  await page.request.post('/api/command', {
    data: { command: 'play', message: { file: WAV, voice: 'beep', loop: true } },
  });

  await page.getByRole('tab', { name: 'Monitor' }).click();
  // A real progress bar appears and its value advances past 0 as the daemon
  // publishes live position (this is the gated W6 telemetry working end-to-end).
  const bar = page.getByLabel(/^progress /);
  await expect(bar).toBeVisible({ timeout: 8_000 });
  await expect
    .poll(async () => Number(await bar.getAttribute('aria-valuenow')), { timeout: 8_000 })
    .toBeGreaterThan(0);

  await page.request.post('/api/command', { data: { command: 'stopall' } });
});
