import { defineConfig, devices } from '@playwright/test';

const visualPort = Number(process.env.OPENBOOK_VISUAL_PORT ?? 5190);

/**
 * Two projects:
 * - `visual`: no backend. `page.route` fixtures answer /config.json and /api/**, and the suite
 *   screenshots each main screen. It starts its own `vite preview` server.
 * - `flows`: the real end-to-end suite. It needs the full local stack (see docs/local-dev.md) and
 *   reads the base URL from E2E_BASE_URL (default http://localhost:5180).
 *
 * Run one project with `npx playwright test --project=visual` or `--project=flows`.
 */
export default defineConfig({
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: { trace: 'retain-on-failure' },
  projects: [
    {
      name: 'visual',
      testDir: './e2e/visual',
      use: { ...devices['Desktop Chrome'], baseURL: `http://127.0.0.1:${visualPort}` },
    },
    {
      name: 'flows',
      testDir: './e2e/flows',
      timeout: 90_000,
      expect: { timeout: 15_000 },
      workers: 1,
      use: {
        ...devices['Desktop Chrome'],
        baseURL: process.env.E2E_BASE_URL ?? 'http://localhost:5180',
        // The local emulator uses a certificate that Node trusts through NODE_EXTRA_CA_CERTS.
        // The browser never talks to it directly: the Vite proxy does.
      },
    },
  ],
  webServer: {
    command: 'npm run build && npm run preview -- --host 127.0.0.1',
    env: { OPENBOOK_WEB_PORT: String(visualPort) },
    url: `http://127.0.0.1:${visualPort}/`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
