import { describe, expect, it } from 'vitest';
import real from '../../../../evals/benchmarks.json';
import fixture from './benchmarks.fixture.json';
import { BenchmarksError, parseBenchmarks, percent, toLanding } from './parse';

// The fixture is for the tests only. The build reads evals/benchmarks.json (see vite.config.ts).
describe('parseBenchmarks', () => {
  it('reads the real evals/benchmarks.json, so a schema change fails here and at build time', () => {
    const b = parseBenchmarks(real);
    expect(b.questions).toBeGreaterThan(0);
    expect(b.corpora.length).toBeGreaterThan(0);
    const landing = toLanding(b);
    expect(landing.recallAt8.hybrid).toBe(b.retrieval.hybrid.recallAt8);
    expect(landing.recallAt1).toBe(b.retrieval.hybrid.recallAt1);
  });

  it('reads the fixture', () => {
    const b = parseBenchmarks(fixture);
    expect(b.questions).toBe(180);
    expect(b.corpora.map((c) => c.id)).toEqual(['college-algebra', 'american-government']);
    expect(b.retrieval.hybrid.recallAt8).toBeGreaterThan(b.retrieval.dense.recallAt8);
    expect(b.gate.falseGateRate).toBe(0);
  });

  it('names the path of the first problem', () => {
    const broken = structuredClone(fixture) as unknown as { retrieval: { combined: { dense: { mrr: unknown } } } };
    broken.retrieval.combined.dense.mrr = 'high';
    expect(() => parseBenchmarks(broken)).toThrow(/retrieval\.combined\.dense\.mrr must be a number/);
  });

  it('fails on a renamed key, another schema version, and a value outside 0 to 1', () => {
    const renamed = structuredClone(fixture) as unknown as { gate: { combined: Record<string, unknown> } };
    renamed.gate.combined.offTopic = renamed.gate.combined.offTopicCaught;
    delete renamed.gate.combined.offTopicCaught;
    expect(() => parseBenchmarks(renamed)).toThrow(/gate\.combined\.offTopicCaught/);
    expect(() => parseBenchmarks({ ...fixture, schema: 2 })).toThrow(BenchmarksError);
    const odd = structuredClone(fixture) as unknown as { gate: { combined: { accuracy: number } } };
    odd.gate.combined.accuracy = 97;
    expect(() => parseBenchmarks(odd)).toThrow(/gate\.combined\.accuracy must be between 0 and 1/);
  });
});

describe('toLanding', () => {
  it('keeps only what the page shows, and finds the publisher in the titles', () => {
    const b = parseBenchmarks(fixture);
    b.corpora = b.corpora.map((c) => ({ ...c, title: `${c.title} (OpenStax)` }));
    expect(toLanding(b)).toEqual({
      questions: 180,
      textbooks: 2,
      publisher: 'OpenStax',
      recallAt8: { hybrid: b.retrieval.hybrid.recallAt8, dense: b.retrieval.dense.recallAt8, bm25: b.retrieval.bm25.recallAt8 },
      recallAt1: b.retrieval.hybrid.recallAt1,
      offTopicCaught: b.gate.offTopicCaught,
    });
  });

  it('names no publisher when the titles differ or name none', () => {
    const b = parseBenchmarks(fixture);
    expect(toLanding(b).publisher).toBeNull();
    b.corpora = [
      { ...b.corpora[0], title: 'A (OpenStax)' },
      { ...b.corpora[1], title: 'B (Other Press)' },
    ];
    expect(toLanding(b).publisher).toBeNull();
  });
});

describe('percent', () => {
  it('rounds, and never shows a false 100% or 0%', () => {
    expect(percent(0.9778)).toBe('98%');
    expect(percent(1)).toBe('100%');
    expect(percent(0)).toBe('0%');
    expect(percent(0.998)).toBe('99%');
    expect(percent(0.004)).toBe('<1%');
  });
});
