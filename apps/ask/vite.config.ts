import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';

const REAL_BENCHMARKS = fileURLToPath(new URL('../../evals/benchmarks.json', import.meta.url));
const FIXTURE_BENCHMARKS = fileURLToPath(new URL('./src/benchmarks/benchmarks.fixture.json', import.meta.url));

/**
 * `virtual:benchmarks` is evals/benchmarks.json, read at build time, so the landing page shows the
 * numbers of the commit it is built from. Without the file, a developer machine falls back to the
 * fixture in src/benchmarks. In CI a missing file fails the build: a page must never ship sample numbers.
 */
function benchmarksPlugin(): Plugin {
  const id = 'virtual:benchmarks';
  const resolved = `\0${id}`;
  return {
    name: 'openbook-benchmarks',
    resolveId: (name) => (name === id ? resolved : undefined),
    load(name) {
      if (name !== resolved) return undefined;
      const real = existsSync(REAL_BENCHMARKS);
      if (!real && process.env.CI) {
        this.error('evals/benchmarks.json is missing. The landing page shows its numbers. Run "make eval" to create it, and commit it.');
      }
      const file = real ? REAL_BENCHMARKS : FIXTURE_BENCHMARKS;
      this.addWatchFile(file);
      return `import data from ${JSON.stringify(file)};\nexport default data;\nexport const source = ${JSON.stringify(real ? 'real' : 'fixture')};`;
    },
  };
}

const webPort = Number(process.env.OPENBOOK_WEB_PORT ?? 5173);
const apiPort = process.env.OPENBOOK_API_PORT ?? '8787';
const emulatorPort = process.env.OPENBOOK_EMULATOR_PORT ?? '9297';

/**
 * Serve /config.json from the file named by OPENBOOK_WEB_CONFIG. Each local stack (make dev,
 * make e2e-local) has its own file, so both can run at once. Without the variable, Vite serves
 * public/config.json as usual.
 */
function webConfigPlugin(): Plugin {
  return {
    name: 'openbook-web-config',
    configureServer(server) {
      const file = process.env.OPENBOOK_WEB_CONFIG;
      if (!file) return;
      server.middlewares.use('/config.json', async (_req, res) => {
        try {
          const body = await readFile(file);
          res.setHeader('content-type', 'application/json');
          res.setHeader('cache-control', 'no-store');
          res.end(body);
        } catch {
          res.statusCode = 404;
          res.end('config file not found');
        }
      });
    },
  };
}

export default defineConfig({
  plugins: [react(), tailwindcss(), webConfigPlugin(), benchmarksPlugin()],
  define: {
    // amazon-cognito-identity-js expects a Node-style `global`.
    global: 'globalThis',
  },
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  server: {
    port: webPort,
    strictPort: true,
    // The landing page reads evals/benchmarks.json, which is outside this app.
    fs: { allow: ['../..'] },
    proxy: {
      '/api': {
        target: `http://127.0.0.1:${apiPort}`,
        changeOrigin: true,
      },
      // AWS Emulator: Cognito calls and presigned S3 PUTs. The scripts set
      // NODE_EXTRA_CA_CERTS to the emulator CA, so TLS verification stays on.
      '/__aws': {
        target: `https://127.0.0.1:${emulatorPort}`,
        changeOrigin: true,
        secure: true,
        rewrite: (path) => path.replace(/^\/__aws/, ''),
      },
    },
  },
  preview: {
    port: webPort,
    strictPort: true,
  },
});
