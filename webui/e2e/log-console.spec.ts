import { test, expect } from '@playwright/test';

// Lane A headless smoke: connect through the (route-mocked) daemon and prove the
// /ws log console renders the welcome + log frames over a REAL browser WebSocket.
// (Reconnect/backoff, the gap marker, and the unauthorized path are covered by the
// component test in tests/logconsole.test.tsx with a mock socket.)
test('log console streams /ws welcome + log frames', async ({ page }) => {
  await page.route('**/health', (route) =>
    route.fulfill({ json: { status: 'ok', service: 'mqttaudio', version: '2.0.0' } }),
  );
  await page.route('**/version', (route) =>
    route.fulfill({ json: { name: 'mqttaudio', version: '2.0.0' } }),
  );

  // Every connection sends the welcome then a log line (robust to React
  // StrictMode's dev-mode effect re-subscribe).
  await page.routeWebSocket(/\/ws$/, (ws) => {
    ws.send(JSON.stringify({ type: 'connected', message: 'Connected', version: '2.0.0' }));
    ws.send(JSON.stringify({ type: 'log', message: 'hello from the daemon' }));
  });

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();

  await expect(page.getByText('Log stream')).toBeVisible();
  await expect(page.getByText('hello from the daemon')).toBeVisible();
  await expect(page.getByText('daemon v2.0.0')).toBeVisible();
});
