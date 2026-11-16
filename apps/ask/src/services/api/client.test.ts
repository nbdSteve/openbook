import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, apiJson, apiRequest, setTokenProvider, setUnauthenticatedHandler, sha256Hex } from './client';
import { describeApiError } from './errors';
import { createCourse, deleteCourse, getMe, rotateInviteCode } from './routes';

const EMPTY_SHA = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
}

let fetchMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  fetchMock = vi.fn(async () => json({}));
  vi.stubGlobal('fetch', fetchMock);
  setTokenProvider(async () => 'TOKEN');
  setUnauthenticatedHandler(() => {});
});

afterEach(() => vi.unstubAllGlobals());

function lastRequest() {
  const [url, init] = fetchMock.mock.calls.at(-1) as [string, RequestInit];
  return { url, init, headers: new Headers(init.headers) };
}

describe('sha256Hex', () => {
  it('matches known vectors in lowercase hex', async () => {
    expect(await sha256Hex(new Uint8Array())).toBe(EMPTY_SHA);
    expect(await sha256Hex(new TextEncoder().encode('abc'))).toBe(
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
    );
  });
});

describe('apiRequest headers', () => {
  it('sends x-auth-token and no body headers on GET', async () => {
    await getMe();
    const { headers, init } = lastRequest();
    expect(headers.get('x-auth-token')).toBe('TOKEN');
    expect(headers.has('x-amz-content-sha256')).toBe(false);
    expect(headers.has('content-type')).toBe(false);
    expect(init.method).toBe('GET');
  });

  it('hashes the exact JSON bytes it sends, including non-ASCII text', async () => {
    await createCourse({ name: 'Café Ünïcode 学' });
    const { headers, init } = lastRequest();
    const sent = init.body as Uint8Array<ArrayBuffer>;
    expect(new TextDecoder().decode(sent)).toBe('{"name":"Café Ünïcode 学"}');
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.get('x-amz-content-sha256')).toBe(await sha256Hex(sent));
  });

  it('sends the empty-string hash for POST and DELETE without a body', async () => {
    fetchMock.mockResolvedValueOnce(json({ inviteCode: 'ABCD2345' }));
    await rotateInviteCode('C1');
    expect(lastRequest().headers.get('x-amz-content-sha256')).toBe(EMPTY_SHA);
    expect(lastRequest().headers.has('content-type')).toBe(false);

    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await deleteCourse('C1');
    expect(lastRequest().headers.get('x-amz-content-sha256')).toBe(EMPTY_SHA);
  });

  it('leaves out x-auth-token when there is no token', async () => {
    setTokenProvider(async () => null);
    await apiRequest('/api/health');
    expect(lastRequest().headers.has('x-auth-token')).toBe(false);
  });

  it('returns undefined for 204 responses', async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    expect(await apiJson('/api/x', { method: 'DELETE' })).toBeUndefined();
  });
});

describe('API error mapping', () => {
  it('parses the contract error shape into an ApiError', async () => {
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'limit_reached', message: 'Too many courses' } }, 409));
    const error = await getMe().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 409, code: 'limit_reached', message: 'Too many courses' });
  });

  it('falls back to a code from the status when the body is not the contract shape', async () => {
    fetchMock.mockResolvedValueOnce(new Response('<html>Bad gateway</html>', { status: 503 }));
    const error = (await getMe().catch((e: unknown) => e)) as ApiError;
    expect(error).toMatchObject({ status: 503, code: 'unavailable' });
    fetchMock.mockResolvedValueOnce(new Response('oops', { status: 500 }));
    expect(((await getMe().catch((e: unknown) => e)) as ApiError).code).toBe('internal');
  });

  it('runs the unauthenticated handler on 401 when a token was sent', async () => {
    const handler = vi.fn();
    setUnauthenticatedHandler(handler);
    fetchMock.mockResolvedValueOnce(json({ error: { code: 'unauthenticated', message: 'Expired' } }, 401));
    await getMe().catch(() => undefined);
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['quota_exceeded', 429, /all 50 messages/],
    ['capacity_exhausted', 503, /daily limit for all users/],
    ['model_unavailable', 503, /busy or unavailable/],
    ['limit_reached', 409, /reached a limit/],
    ['too_large', 413, /too large.*25 MB/],
  ])('gives a specific message for %s', (code, status, pattern) => {
    const limits = { dailyMessages: 50, maxCoursesOwned: 10, maxDocumentsPerCourse: 50, maxUploadBytes: 25 * 1024 * 1024 };
    expect(describeApiError(new ApiError(status, code, 'server text'), limits)).toMatch(pattern);
  });
});
