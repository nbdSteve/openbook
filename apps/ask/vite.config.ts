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

/**
 * `virtual:benchmarks` holds the numbers that the landing page shows, read from
 * evals/benchmarks.json at build time, so the page shows the numbers of the commit it is built from.
 * The file is checked here: a missing file, a bad number, or a renamed key fails the build, and the
 * visitor never sees a broken page. There is no fallback to sample numbers.
 */
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

/**
 * `virtual:example-answer` is the fixed example answer of the landing page, as a HAST tree. It is
 * rendered here, at build time, with the same Markdown, math, and KaTeX plugins as the chat. The
 * page turns the tree into React elements (not into an HTML string), so the first view needs
 * neither the Markdown parser nor the style attributes of raw HTML. The tree is our own constant,
 * never model output, so there is nothing to sanitize.
 */
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
      // Positions only make the file larger.
      return `export default ${JSON.stringify(tree, (key, value: unknown) => (key === 'position' ? undefined : value))};`;
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
  plugins: [react(), tailwindcss(), webConfigPlugin(), benchmarksPlugin(), exampleAnswerPlugin()],
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
