import { describe, expect, it } from 'vitest';
import fixture from './benchmarks.fixture.json';
import { BenchmarksError, parseBenchmarks, percent } from './parse';

describe('parseBenchmarks', () => {
  it('reads the fixture, which has the shape of evals/benchmarks.json', () => {
    const b = parseBenchmarks(fixture);
    expect(b.questions).toBe(180);
    expect(b.corpora.map((c) => c.id)).toEqual(['college-algebra', 'american-government']);
    expect(b.retrieval.hybrid.recallAt8).toBeGreaterThan(b.retrieval.dense.recallAt8);
    expect(b.retrieval.hybrid.recallAt1).toBeLessThan(b.retrieval.hybrid.recallAt8);
    expect(b.gate.falseGateRate).toBe(0);
  });

  it('names the path of the first problem', () => {
    const broken = structuredClone(fixture) as unknown as { retrieval: { combined: { dense: { mrr: unknown } } } };
    broken.retrieval.combined.dense.mrr = 'high';
    expect(() => parseBenchmarks(broken)).toThrow(/retrieval\.combined\.dense\.mrr must be a number/);
  });

  it('rejects another schema version and a value outside 0 to 1', () => {
    expect(() => parseBenchmarks({ ...fixture, schema: 2 })).toThrow(BenchmarksError);
    const odd = structuredClone(fixture) as unknown as { gate: { combined: { accuracy: number } } };
    odd.gate.combined.accuracy = 97;
    expect(() => parseBenchmarks(odd)).toThrow(/gate\.combined\.accuracy must be between 0 and 1/);
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
