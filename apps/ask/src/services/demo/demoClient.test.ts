import { afterEach, describe, expect, it, vi } from 'vitest';
import { askDemo, DEMO_PATH, DemoError, describeDemoError } from './demoClient';
import type { ChatEvent } from '@/services/api/types';

const sse = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
const STREAM =
  sse('meta', { conversationId: 'C1', userMessageId: 'u', assistantMessageId: 'a', title: 't', model: 'm' }) +
  sse('delta', { text: 'Hello' }) +
  sse('done', { usage: { inputTokens: 1, outputTokens: 1 }, stopReason: 'end_turn' });

function stubFetch(response: Response | Error | DOMException) {
  const fetchMock = vi.fn(async () => {
    if (!(response instanceof Response)) throw response;
    return response;
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

async function collect(options: Parameters<typeof askDemo>[0]): Promise<ChatEvent[]> {
  const events: ChatEvent[] = [];
  for await (const event of askDemo(options)) events.push(event);
  return events;
}

const errorBody = (code: string, extra: Record<string, unknown> = {}) => JSON.stringify({ error: { code, message: `${code} message`, ...extra } });
// The demo route sets this header on every response it makes.
const FROM_DEMO = { 'x-openbook-demo': '1' };

afterEach(() => vi.unstubAllGlobals());

describe('askDemo', () => {
  it('posts the question with its hash, sends no history, and yields the chat events', async () => {
    const fetchMock = stubFetch(new Response(STREAM, { status: 200 }));
    const events = await collect({ message: 'What is x?' });
    expect(events.map((e) => e.type)).toEqual(['meta', 'delta', 'done']);

    const [path, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe(DEMO_PATH);
    expect(init.method).toBe('POST');
    const headers = init.headers as Record<string, string>;
    expect(headers['x-amz-content-sha256']).toMatch(/^[0-9a-f]{64}$/);
    expect(JSON.parse(new TextDecoder().decode(init.body as Uint8Array))).toEqual({ message: 'What is x?' });
    // No account: the demo never sends a token.
    expect(headers['x-auth-token']).toBeUndefined();
  });

  it('sends the conversation ID of an earlier answer for a follow-up', async () => {
    const fetchMock = stubFetch(new Response(STREAM, { status: 200 }));
    await collect({ message: 'And then?', conversationId: 'C1' });
    const init = (fetchMock.mock.calls[0] as unknown as [string, RequestInit])[1];
    expect(JSON.parse(new TextDecoder().decode(init.body as Uint8Array))).toEqual({ message: 'And then?', conversationId: 'C1' });
  });

  it.each([
    [429, 'demo_rate_limited', { retryAfterSeconds: 42 }, 'rate_limited'],
    [503, 'demo_busy', {}, 'busy'],
    [503, 'demo_unavailable', {}, 'unavailable'],
    [400, 'invalid_request', {}, 'invalid'],
    [404, 'demo_unavailable', {}, 'unavailable'],
  ])('maps %i %s to %s', async (status, code, extra, kind) => {
    stubFetch(new Response(errorBody(code, extra), { status, headers: FROM_DEMO }));
    const error = await collect({ message: 'x' }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(DemoError);
    expect((error as DemoError).kind).toBe(kind);
    if (code === 'demo_rate_limited') expect((error as DemoError).retryAfterSeconds).toBe(42);
  });

  it('treats a 503 without a JSON error body as busy (a CloudFront function rewrites Lambda throttles)', async () => {
    stubFetch(new Response('<html>Service Unavailable</html>', { status: 503, headers: FROM_DEMO }));
    expect(((await collect({ message: 'x' }).catch((e: unknown) => e)) as DemoError).kind).toBe('busy');
  });

  it.each([
    ['Lambda 429 (a throttle)', 429, '{"message":"Rate Exceeded."}'],
    ['CloudFront 502', 502, '<html>Bad Gateway</html>'],
    ['CloudFront 504', 504, '<html>Gateway Timeout</html>'],
    ['a 503 page', 503, '<html>Service Unavailable</html>'],
    ['a 500 with a JSON error from another layer', 500, errorBody('internal')],
  ])('treats %s without the demo header as busy', async (_name, status, body) => {
    stubFetch(new Response(body, { status }));
    const error = (await collect({ message: 'x' }).catch((e: unknown) => e)) as DemoError;
    expect(error).toBeInstanceOf(DemoError);
    expect(error.kind).toBe('busy');
  });

  it('keeps the rate limit of the demo itself (with the header) apart from a throttle', async () => {
    stubFetch(new Response(errorBody('demo_rate_limited', { retryAfterSeconds: 9 }), { status: 429, headers: FROM_DEMO }));
    expect(((await collect({ message: 'x' }).catch((e: unknown) => e)) as DemoError).kind).toBe('rate_limited');
  });

  it('reads Retry-After when the body has no retryAfterSeconds', async () => {
    stubFetch(new Response(errorBody('demo_rate_limited'), { status: 429, headers: { ...FROM_DEMO, 'retry-after': '30' } }));
    const error = (await collect({ message: 'x' }).catch((e: unknown) => e)) as DemoError;
    expect(error.retryAfterSeconds).toBe(30);
  });

  it('reports a network failure, and lets an abort through', async () => {
    stubFetch(new TypeError('Failed to fetch'));
    expect(((await collect({ message: 'x' }).catch((e: unknown) => e)) as DemoError).kind).toBe('network');
    stubFetch(new DOMException('aborted', 'AbortError'));
    expect(await collect({ message: 'x' }).catch((e: unknown) => e)).toBeInstanceOf(DOMException);
  });
});

describe('describeDemoError', () => {
  it('speaks plainly to a visitor', () => {
    expect(describeDemoError(new DemoError('rate_limited', '', 42))).toBe('You are asking fast. Try again in 42 seconds.');
    expect(describeDemoError(new DemoError('rate_limited', '', 300))).toBe('You are asking fast. Try again in 5 minutes.');
    expect(describeDemoError(new DemoError('rate_limited', ''))).toMatch(/Give it a minute/);
    expect(describeDemoError(new DemoError('busy', ''))).toMatch(/today/);
    expect(describeDemoError(new DemoError('unavailable', ''))).toMatch(/resting/);
    expect(describeDemoError(new DemoError('invalid', 'Too long.'))).toBe('Too long.');
    expect(describeDemoError(new Error('boom'))).toMatch(/Try again/);
  });
});
