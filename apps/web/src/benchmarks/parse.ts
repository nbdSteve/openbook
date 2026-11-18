
export type RetrievalMode = 'hybrid' | 'dense' | 'bm25';

export interface RetrievalMetrics {
  recallAt1: number;
  recallAt5: number;
  recallAt8: number;
  ndcgAt8: number;
  mrr: number;
}

export interface GateMetrics {
  accuracy: number;
  falseGateRate: number;
  offTopicCaught: number;
  notCoveredCaught: number;
}

export interface Corpus {
  id: string;
  title: string;
  licence: string;
  source: string;
  pages: number;
  questions: number;
}

export interface Benchmarks {
  embeddingModel: string;
  questions: number;
  corpora: Corpus[];
  retrieval: Record<RetrievalMode, RetrievalMetrics>;
  gate: GateMetrics;
}

export class BenchmarksError extends Error {
  constructor(message: string) {
    super(`evals/benchmarks.json: ${message}`);
    this.name = 'BenchmarksError';
  }
}

const SCHEMA = 1;

function obj(value: unknown, path: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new BenchmarksError(`${path} must be an object`);
  return value as Record<string, unknown>;
}

function num(value: unknown, path: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) throw new BenchmarksError(`${path} must be a number`);
  return value;
}

function str(value: unknown, path: string): string {
  if (typeof value !== 'string' || value === '') throw new BenchmarksError(`${path} must be a non-empty string`);
  return value;
}

function ratio(value: unknown, path: string): number {
  const n = num(value, path);
  if (n < 0 || n > 1) throw new BenchmarksError(`${path} must be between 0 and 1`);
  return n;
}

function retrieval(value: unknown, path: string): RetrievalMetrics {
  const v = obj(value, path);
  return {
    recallAt1: ratio(v.recallAt1, `${path}.recallAt1`),
    recallAt5: ratio(v.recallAt5, `${path}.recallAt5`),
    recallAt8: ratio(v.recallAt8, `${path}.recallAt8`),
    ndcgAt8: ratio(v.ndcgAt8, `${path}.ndcgAt8`),
    mrr: ratio(v.mrr, `${path}.mrr`),
  };
}

function gate(value: unknown, path: string): GateMetrics {
  const v = obj(value, path);
  return {
    accuracy: ratio(v.accuracy, `${path}.accuracy`),
    falseGateRate: ratio(v.falseGateRate, `${path}.falseGateRate`),
    offTopicCaught: ratio(v.offTopicCaught, `${path}.offTopicCaught`),
    notCoveredCaught: ratio(v.notCoveredCaught, `${path}.notCoveredCaught`),
  };
}

export function parseBenchmarks(raw: unknown): Benchmarks {
  const v = obj(raw, 'file');
  if (v.schema !== SCHEMA) throw new BenchmarksError(`schema must be ${SCHEMA}, found ${String(v.schema)}`);
  const corpora = Array.isArray(v.corpora) ? v.corpora : [];
  if (corpora.length === 0) throw new BenchmarksError('corpora must list at least one corpus');
  const combined = obj(obj(v.retrieval, 'retrieval').combined, 'retrieval.combined');
  return {
    embeddingModel: str(v.embeddingModel, 'embeddingModel'),
    questions: num(v.questions, 'questions'),
    corpora: corpora.map((c, i) => {
      const o = obj(c, `corpora[${i}]`);
      return {
        id: str(o.id, `corpora[${i}].id`),
        title: str(o.title, `corpora[${i}].title`),
        licence: str(o.licence, `corpora[${i}].licence`),
        source: str(o.source, `corpora[${i}].source`),
        pages: num(o.pages, `corpora[${i}].pages`),
        questions: num(o.questions, `corpora[${i}].questions`),
      };
    }),
    retrieval: {
      hybrid: retrieval(combined.hybrid, 'retrieval.combined.hybrid'),
      dense: retrieval(combined.dense, 'retrieval.combined.dense'),
      bm25: retrieval(combined.bm25, 'retrieval.combined.bm25'),
    },
    gate: gate(obj(v.gate, 'gate').combined, 'gate.combined'),
  };
}

export function percent(value: number): string {
  const p = value * 100;
  if (value > 0 && p < 1) return '<1%';
  if (value < 1 && p >= 99.5) return '99%';
  return `${Math.round(p)}%`;
}

export interface LandingBenchmarks {
  questions: number;
  textbooks: number;
  publisher: string | null;
  recallAt8: Record<RetrievalMode, number>;
  recallAt1: number;
  offTopicCaught: number;
}

export function toLanding(b: Benchmarks): LandingBenchmarks {
  const publishers = b.corpora.map((c) => /\(([^)]+)\)\s*$/.exec(c.title)?.[1] ?? null);
  const same = publishers.every((p) => p !== null && p === publishers[0]);
  return {
    questions: b.questions,
    textbooks: b.corpora.length,
    publisher: same ? publishers[0] : null,
    recallAt8: { hybrid: b.retrieval.hybrid.recallAt8, dense: b.retrieval.dense.recallAt8, bm25: b.retrieval.bm25.recallAt8 },
    recallAt1: b.retrieval.hybrid.recallAt1,
    offTopicCaught: b.gate.offTopicCaught,
  };
}
