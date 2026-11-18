import { createHash } from 'node:crypto';
import { defineConfig, devices } from '@playwright/test';

const portForThisCheckout = 5200 + (createHash('sha1').update(process.cwd()).digest().readUInt16BE(0) % 500);
const port = Number(process.env.OPENBOOK_VISUAL_PORT ?? portForThisCheckout);

export default defineConfig({
  testDir: './e2e/visual',
  fullyParallel: true,
  timeout: 60_000,
  expect: { timeout: 20_000 },
  workers: 4,
  forbidOnly: !!process.env.CI,
  reporter: 'list',
  use: { ...devices['Desktop Chrome'], baseURL: `http://127.0.0.1:${port}`, trace: 'retain-on-failure' },
  projects: [{ name: 'visual' }],
  webServer: {
    command: 'npm run build && npm run preview -- --host 127.0.0.1',
    env: { OPENBOOK_WEB_PORT: String(port) },
    url: `http://127.0.0.1:${port}/`,
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
