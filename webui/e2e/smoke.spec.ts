import { test, expect } from '@playwright/test';

// Headless smoke (Lane A): load the app against a route-mocked daemon and prove
// the connect/bootstrap surface reads /health + /version. No real daemon.
test('connects to a mocked daemon and shows its identity', async ({ page }) => {
  await page.route('**/health', (route) =>
    route.fulfill({ json: { status: 'ok', service: 'mqttaudio', version: '2.0.0' } }),
  );
  await page.route('**/version', (route) =>
    route.fulfill({ json: { name: 'mqttaudio', version: '2.0.0' } }),
  );

  await page.goto('/');
  await page.getByRole('button', { name: /connect/i }).click();

  await expect(page.getByText(/connected:/i)).toBeVisible();
  await expect(page.getByText(/v2\.0\.0/)).toBeVisible();
});
