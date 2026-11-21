import { test, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

// Lane A a11y gate (Sprint W9): no serious/critical axe violations on the connect
// screen or the connected dashboard (against a route-mocked daemon).
async function noSeriousViolations(page: import('@playwright/test').Page) {
  const results = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa'])
    .analyze();
  const serious = results.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(serious, JSON.stringify(serious.map((v) => v.id), null, 2)).toEqual([]);
}

test('the connect screen has no serious a11y violations', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).waitFor();
  await noSeriousViolations(page);
});

test('the connected dashboard has no serious a11y violations', async ({ page }) => {
  await page.route('**/health', (route) =>
    route.fulfill({ json: { status: 'ok', service: 'mqttaudio', version: '2.0.0' } }),
  );
  await page.route('**/version', (route) => route.fulfill({ json: { name: 'mqttaudio', version: '2.0.0' } }));
  await page.routeWebSocket(/\/ws$/, (ws) => {
    ws.send(JSON.stringify({ type: 'connected', version: '2.0.0' }));
  });

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();
  await page.getByText('Log stream').waitFor();
  await noSeriousViolations(page);
});
