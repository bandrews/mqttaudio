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
