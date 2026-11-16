import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { fileURLToPath } from 'node:url';

const webPort = Number(process.env.OPENBOOK_WEB_PORT ?? 5173);
const apiPort = process.env.OPENBOOK_API_PORT ?? '8787';
const emulatorPort = process.env.OPENBOOK_EMULATOR_PORT ?? '9297';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  define: {
    // amazon-cognito-identity-js expects a Node-style `global`.
    global: 'globalThis',
  },
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  build: {
    // The syntax highlighter chunk is large but loads only when an answer has a code block.
    chunkSizeWarningLimit: 700,
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
