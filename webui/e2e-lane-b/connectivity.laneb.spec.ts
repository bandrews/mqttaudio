import { test, expect } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

// Lane B: drive the SPA through the dev proxy against a REAL daemon, and verify
// connection + restart survival (the daemon is spawned/killed by this spec).
// NOTE: real /ws log LINES are gated on a daemon gap (WebSocketLogLayer not wired;
// see docs/bugs.md, Sprint W1) — this verifies the welcome frame, the live state,
// and the backoff reconnect, which are the connectivity claims.

const BIN = resolve(process.cwd(), '..', 'target', 'release', 'mqttaudio');
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

test.afterAll(() => {
  daemon?.kill('SIGKILL');
});

test('connects to a real daemon through the proxy and survives a restart', async ({ page }) => {
  daemon = await startDaemon();

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();

  // Connected view + log console mount; the real welcome frame reports the version.
  await expect(page.getByText('Log stream')).toBeVisible();
  await expect(page.getByText('daemon v2.0.0')).toBeVisible({ timeout: 20_000 });
  await expect(page.getByText('offline — reconnecting')).toHaveCount(0);

  // Kill the daemon: the socket closes, the /version re-probe fails, the console
  // drops to offline (not unauthorized — the daemon is open).
  daemon.kill('SIGKILL');
  await waitDaemonDown();
  await expect(page.getByText('offline — reconnecting')).toBeVisible({ timeout: 20_000 });

  // Bring it back: the backoff reconnect re-subscribes and marks the gap.
  daemon = await startDaemon();
  await expect(page.getByText(/messages during the gap were dropped/i).first()).toBeVisible({
    timeout: 45_000,
  });
  await expect(page.getByText('daemon v2.0.0')).toBeVisible();
});
