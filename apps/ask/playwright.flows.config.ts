import { defineConfig, devices } from '@playwright/test';

/**
 * The real end-to-end suite. It needs the full local stack (see docs/local-dev.md) and starts
 * nothing itself. The base URL comes from E2E_BASE_URL (default http://localhost:5180).
 * `scripts/e2e-local.sh` runs it with: npm --prefix apps/ask run e2e -- --project=flows
 */
export default defineConfig({
  testDir: './e2e/flows',
  timeout: 90_000,
  expect: { timeout: 15_000 },
  workers: 1,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    ...devices['Desktop Chrome'],
    baseURL: process.env.E2E_BASE_URL ?? 'http://localhost:5180',
    trace: 'retain-on-failure',
  },
  projects: [{ name: 'flows' }],
});
