import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { existsSync, readFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';
import rehypeKatex from 'rehype-katex';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import remarkParse from 'remark-parse';
import remarkRehype from 'remark-rehype';
import { unified } from 'unified';
import { parseBenchmarks, toLanding } from './src/benchmarks/parse.ts';
import { EXAMPLE_MARKDOWN } from './src/pages/landing/exampleContent.ts';

const BENCHMARKS_FILE = fileURLToPath(new URL('../../evals/benchmarks.json', import.meta.url));

function benchmarksPlugin(): Plugin {
  const id = 'virtual:benchmarks';
  const resolved = `\0${id}`;
  const fix = 'Create it with "cargo run -p openbook-eval -- offline --write-baseline" and commit it.';
  return {
    name: 'openbook-benchmarks',
    resolveId: (name) => (name === id ? resolved : undefined),
    load(name) {
      if (name !== resolved) return undefined;
      if (!existsSync(BENCHMARKS_FILE)) this.error(`evals/benchmarks.json is missing. The landing page shows its numbers. ${fix}`);
      this.addWatchFile(BENCHMARKS_FILE);
      try {
        return `export default ${JSON.stringify(toLanding(parseBenchmarks(JSON.parse(readFileSync(BENCHMARKS_FILE, 'utf8')))))};`;
      } catch (error) {
        this.error(`${error instanceof Error ? error.message : String(error)}. Re-create the file: ${fix}`);
      }
    },
  };
}

function exampleAnswerPlugin(): Plugin {
  const id = 'virtual:example-answer';
  const resolved = `\0${id}`;
  return {
    name: 'openbook-example-answer',
    resolveId: (name) => (name === id ? resolved : undefined),
    async load(name) {
      if (name !== resolved) return undefined;
      const processor = unified().use(remarkParse).use(remarkGfm).use(remarkMath).use(remarkRehype).use(rehypeKatex);
      const tree = await processor.run(processor.parse(EXAMPLE_MARKDOWN));
      return `export default ${JSON.stringify(tree, (key, value: unknown) => (key === 'position' ? undefined : value))};`;
    },
  };
}

const webPort = Number(process.env.OPENBOOK_WEB_PORT ?? 5173);
const apiPort = process.env.OPENBOOK_API_PORT ?? '8787';
const emulatorPort = process.env.OPENBOOK_EMULATOR_PORT ?? '9297';

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
  plugins: [react(), tailwindcss(), webConfigPlugin(), benchmarksPlugin(), exampleAnswerPlugin()],
  define: {
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
    fs: { allow: ['../..'] },
    proxy: {
      '/api': {
        target: `http://127.0.0.1:${apiPort}`,
        changeOrigin: true,
      },
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
