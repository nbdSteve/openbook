import { defineConfig, devices } from '@playwright/test';

const port = Number(process.env.OPENBOOK_VISUAL_PORT ?? 5190);

/**
 * The visual suite needs no backend. `page.route` fixtures answer /config.json and /api/**. It
 * builds the app and starts its own `vite preview` server, so it always tests this checkout.
 */
export default defineConfig({
  testDir: './e2e/visual',
  fullyParallel: true,
  // The screens load lazy chunks and fonts. A busy machine needs more than the 5 s default.
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
    // Never reuse a server: another checkout could be listening on the port.
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
