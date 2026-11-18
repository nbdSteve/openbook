import landing from 'virtual:benchmarks';

export type { LandingBenchmarks, RetrievalMode } from './parse';
export { percent } from './parse';

/**
 * The retrieval numbers of this build. The Vite plugin `benchmarksPlugin` (vite.config.ts) reads
 * evals/benchmarks.json, checks it with `parseBenchmarks`, and emits only the numbers that the
 * landing page shows. A missing file or a changed schema fails the build, never the visitor's page.
 */
export const benchmarks = landing;

/** The method, on GitHub. */
export const METHOD_URL = 'https://github.com/nbdSteve/openbook/blob/main/docs/evaluation.md';
