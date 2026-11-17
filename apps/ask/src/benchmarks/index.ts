import raw, { source } from 'virtual:benchmarks';
import { parseBenchmarks } from './parse';

export type { Benchmarks, RetrievalMode } from './parse';
export { percent } from './parse';

/**
 * The retrieval numbers of this build. The build reads evals/benchmarks.json (see
 * `benchmarksPlugin` in vite.config.ts), so the page shows the numbers of the deployed commit.
 * `source` is `fixture` only on a machine without the file. In CI a missing file fails the build.
 */
export const benchmarks = parseBenchmarks(raw);
export const benchmarksSource: 'real' | 'fixture' = source;

/** The method, on GitHub. */
export const METHOD_URL = 'https://github.com/nbdSteve/openbook/blob/main/docs/evaluation.md';
