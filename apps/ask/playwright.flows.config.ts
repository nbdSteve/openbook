import { defineConfig, devices } from '@playwright/test';

/**
 * The real end-to-end suite. It starts nothing itself. It needs the full local stack
 * (see docs/local-dev.md), or a deployed stack with E2E_TARGET=deployed. The base URL comes from E2E_BASE_URL (default http://localhost:5180).
 * `scripts/e2e-local.sh` runs it with: npm --prefix apps/ask run e2e -- --project=flows
 */
export default defineConfig({
  testDir: './e2e/flows',
  // A real model and a real Cognito are slower than the local fakes.
  timeout: process.env.E2E_TARGET === 'deployed' ? 180_000 : 90_000,
  globalTeardown: './e2e/flows/global-teardown.ts',
  expect: { timeout: 15_000 },
  workers: 1,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    ...devices['Desktop Chrome'],
    baseURL: process.env.E2E_BASE_URL ?? 'http://localhost:5180',
    // Off on purpose: the flows share contexts across serial tests, and the built-in trace fixture
    // can hang on them. helpers.ts records a trace per context instead.
    trace: 'off',
  },
  projects: [{ name: 'flows' }],
});
