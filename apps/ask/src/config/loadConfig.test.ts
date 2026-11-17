import { describe, expect, it, vi } from 'vitest';
import { ConfigError, loadConfig, parseConfig, parseDemo } from './loadConfig';

const valid = {
  stage: 'dev',
  region: 'ap-southeast-2',
  cognito: { userPoolId: 'ap-southeast-2_abc', clientId: 'client', endpoint: null },
};

const respond = (body: BodyInit, status = 200) => vi.fn(async () => new Response(body, { status })) as unknown as typeof fetch;

describe('loadConfig', () => {
  it('loads and returns the config', async () => {
    const fetchFn = respond(JSON.stringify(valid));
    expect(await loadConfig(fetchFn)).toEqual(valid);
    expect(fetchFn).toHaveBeenCalledWith('/config.json', { cache: 'no-store' });
  });

  it('keeps a local Cognito endpoint', async () => {
    const config = { ...valid, cognito: { ...valid.cognito, endpoint: 'http://localhost:5173/__aws/' } };
    expect((await loadConfig(respond(JSON.stringify(config)))).cognito.endpoint).toBe('http://localhost:5173/__aws/');
  });

  it('treats a missing endpoint as null', () => {
    const { endpoint: _unused, ...cognito } = valid.cognito;
    void _unused;
    expect(parseConfig({ ...valid, cognito }).cognito.endpoint).toBeNull();
  });

  it('fails with a clear error when the file is missing', async () => {
    await expect(loadConfig(respond('nope', 404))).rejects.toThrow(/status 404/);
  });

  it('fails when the server returns the HTML page instead of JSON', async () => {
    await expect(loadConfig(respond('<!doctype html>'))).rejects.toThrow(/not valid JSON/);
  });

  it('fails when the network is down', async () => {
    const down = vi.fn(async () => {
      throw new TypeError('Failed to fetch');
    }) as unknown as typeof fetch;
    await expect(loadConfig(down)).rejects.toBeInstanceOf(ConfigError);
  });

  it('fails when required fields are missing', async () => {
    await expect(loadConfig(respond(JSON.stringify({ stage: 'dev' })))).rejects.toThrow(/wrong shape/);
    await expect(loadConfig(respond(JSON.stringify({ ...valid, cognito: { userPoolId: 'x' } })))).rejects.toThrow(/missing/);
  });
});

describe('parseDemo', () => {
  it('reads an enabled demo block', () => {
    const demo = parseDemo({
      enabled: true,
      sources: [{ title: 'College Algebra', license: 'CC BY 4.0', url: 'https://openstax.org/details/books/college-algebra' }, { nope: 1 }],
      examples: ['Factor x^2 - 5x + 6', 7],
      model: { label: 'Claude Sonnet 4.6' },
    });
    expect(demo).toEqual({
      enabled: true,
      sources: [{ title: 'College Algebra', license: 'CC BY 4.0', url: 'https://openstax.org/details/books/college-algebra' }],
      examples: ['Factor x^2 - 5x + 6'],
      model: { label: 'Claude Sonnet 4.6' },
    });
  });

  it('turns the demo off when it is disabled, missing, or not an object', () => {
    expect(parseDemo({ enabled: false, sources: [], examples: [] })).toBeUndefined();
    expect(parseDemo(undefined)).toBeUndefined();
    expect(parseDemo('yes')).toBeUndefined();
  });

  it('keeps the config valid when the demo block is bad', () => {
    expect(parseConfig({ ...valid, demo: 12 }).demo).toBeUndefined();
  });
});
