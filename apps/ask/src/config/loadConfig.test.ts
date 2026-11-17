import { describe, expect, it, vi } from 'vitest';
import { ConfigError, loadConfig, parseConfig } from './loadConfig';

const valid = {
  stage: 'dev',
  region: 'ap-southeast-2',
  cognito: { userPoolId: 'ap-southeast-2_abc', clientId: 'client', endpoint: null },
};

const respond = (body: BodyInit, status = 200) => vi.fn(async () => new Response(body, { status })) as unknown as typeof fetch;

describe('loadConfig', () => {
  it('loads and returns the config', async () => {
    const fetchFn = respond(JSON.stringify(valid));
    expect(await loadConfig(fetchFn)).toEqual({ ...valid, demo: { enabled: false, model: { label: '' }, sources: [], examples: [] } });
    expect(fetchFn).toHaveBeenCalledWith('/config.json', { cache: 'no-store' });
  });

  it('reads the demo settings, and an absent or off demo is off', () => {
    const demo = {
      enabled: true,
      model: { label: 'Claude Sonnet 4.6' },
      sources: [{ title: 'College Algebra', license: 'CC BY 4.0', url: 'https://openstax.org/details/books/college-algebra' }],
      examples: ['What is a function?', 'What is federalism?'],
    };
    expect(parseConfig({ ...valid, demo }).demo).toEqual(demo);
    expect(parseConfig({ ...valid, demo: { ...demo, enabled: false } }).demo).toEqual({ enabled: false, model: { label: '' }, sources: [], examples: [] });
    expect(parseConfig({ ...valid, demo: 'yes' }).demo.enabled).toBe(false);
    // Entries that are not well formed are dropped, not trusted.
    const messy = { enabled: true, sources: [{ title: 'x' }, demo.sources[0]], examples: ['ok', 3, ''] };
    expect(parseConfig({ ...valid, demo: messy }).demo).toEqual({ enabled: true, model: { label: '' }, sources: demo.sources, examples: ['ok'] });
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
