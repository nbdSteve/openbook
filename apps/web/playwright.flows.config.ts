import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './e2e/flows',
  timeout: process.env.E2E_TARGET === 'deployed' ? 180_000 : 90_000,
  globalTeardown: './e2e/flows/global-teardown.ts',
  expect: { timeout: 15_000 },
  workers: 1,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    ...devices['Desktop Chrome'],
    baseURL: process.env.E2E_BASE_URL ?? 'http://localhost:5180',
    trace: 'off',
  },
  projects: [{ name: 'flows' }],
});
