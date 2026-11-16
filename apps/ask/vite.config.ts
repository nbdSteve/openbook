import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';

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
  plugins: [react(), tailwindcss(), webConfigPlugin()],
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
