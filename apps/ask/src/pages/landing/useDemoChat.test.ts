import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DEMO_MAX_TURNS } from '@/services/demo/demoClient';
import { useDemoChat } from './useDemoChat';

const sse = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
const answer = (conversationId: string, text: string, withSources = true) =>
  sse('meta', { conversationId, userMessageId: 'u', assistantMessageId: 'a', title: 't', model: 'm' }) +
  (withSources ? sse('sources', { sources: [{ n: 1, documentId: 'd', documentName: 'Book', page: 3, snippet: 's' }] }) : '') +
  sse('delta', { text }) +
  sse('done', { usage: { inputTokens: 1, outputTokens: 1 }, stopReason: 'end_turn' });

const json = (status: number, code: string, extra: Record<string, unknown> = {}) =>
  new Response(JSON.stringify({ error: { code, message: code, ...extra } }), { status });

/** The bodies that the fake endpoint sends, in order. Each call also records the request body. */
function fakeEndpoint(responses: (Response | (() => Response))[]) {
  const bodies: { message: string; conversationId?: string }[] = [];
  const queue = [...responses];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (_path: string, init: RequestInit) => {
      bodies.push(JSON.parse(new TextDecoder().decode(init.body as Uint8Array)));
      const next = queue.shift();
      if (!next) throw new Error('no response left');
      return typeof next === 'function' ? next() : next;
    }),
  );
  return bodies;
}

afterEach(() => vi.unstubAllGlobals());

describe('useDemoChat', () => {
  it('shows the question and the streamed answer with its sources', async () => {
    fakeEndpoint([new Response(answer('C1', 'Use the formula [1].'))]);
    const { result } = renderHook(() => useDemoChat());
    await act(async () => {
      expect(await result.current.ask('  How?  ')).toBe(true);
    });
    expect(result.current.status).toBe('idle');
    expect(result.current.messages.map((m) => [m.role, m.content])).toEqual([
      ['user', 'How?'],
      ['assistant', 'Use the formula [1].'],
    ]);
    expect(result.current.messages[1].sources).toHaveLength(1);
    expect(result.current.messages[1].state).toBeUndefined();
  });

  it('keeps the conversation for a follow-up, and Start over forgets it', async () => {
    const bodies = fakeEndpoint([new Response(answer('C1', 'One')), new Response(answer('C1', 'Two')), new Response(answer('C2', 'Three'))]);
    const { result } = renderHook(() => useDemoChat());
    await act(async () => void (await result.current.ask('first')));
    await act(async () => void (await result.current.ask('second')));
    expect(bodies[1]).toEqual({ message: 'second', conversationId: 'C1' });
    expect(result.current.messages).toHaveLength(4);

    act(() => result.current.reset());
    expect(result.current.messages).toEqual([]);
    await act(async () => void (await result.current.ask('third')));
    expect(bodies[2]).toEqual({ message: 'third' });
  });

  it('starts a new conversation after the turn limit and says so', async () => {
    const responses = Array.from({ length: DEMO_MAX_TURNS + 1 }, (_, i) => new Response(answer(i < DEMO_MAX_TURNS ? 'OLD' : 'NEW', `Answer ${i}`)));
    const bodies = fakeEndpoint(responses);
    const { result } = renderHook(() => useDemoChat());
    for (let i = 0; i < DEMO_MAX_TURNS; i++) await act(async () => void (await result.current.ask(`q${i}`)));
    expect(result.current.notice).toBeNull();
    await act(async () => void (await result.current.ask('one more')));
    expect(bodies[DEMO_MAX_TURNS]).toEqual({ message: 'one more' });
    expect(result.current.notice).toMatch(/fresh one/);
    // The page starts empty again: only the new turn shows.
    expect(result.current.messages.map((m) => m.content)).toEqual(['one more', `Answer ${DEMO_MAX_TURNS}`]);
  });

  it('starts a new conversation once when the server forgot the old one', async () => {
    const bodies = fakeEndpoint([new Response(answer('C1', 'One')), json(404, 'not_found'), new Response(answer('C2', 'Fresh'))]);
    const { result } = renderHook(() => useDemoChat());
    await act(async () => void (await result.current.ask('first')));
    await act(async () => void (await result.current.ask('second')));
    expect(bodies.map((b) => b.conversationId)).toEqual([undefined, 'C1', undefined]);
    expect(result.current.notice).toMatch(/ended/);
    expect(result.current.problem).toBeNull();
    expect(result.current.messages.map((m) => m.content)).toEqual(['second', 'Fresh']);
  });

  it('takes the pending messages back and names the problem when the demo refuses', async () => {
    fakeEndpoint([json(429, 'demo_rate_limited', { retryAfterSeconds: 42 })]);
    const { result } = renderHook(() => useDemoChat());
    await act(async () => {
      expect(await result.current.ask('too fast')).toBe(false);
    });
    expect(result.current.messages).toEqual([]);
    expect(result.current.problem).toEqual({ kind: 'rate_limited', message: 'You are asking fast. Try again in 42 seconds.' });
    expect(result.current.status).toBe('idle');
  });

  it('marks the answer failed when the stream ends with an error event', async () => {
    fakeEndpoint([new Response(sse('meta', { conversationId: 'C1', userMessageId: 'u', assistantMessageId: 'a', title: 't', model: 'm' }) + sse('delta', { text: 'Part' }) + sse('error', { code: 'model_unavailable', message: 'Busy.' }))]);
    const { result } = renderHook(() => useDemoChat());
    await act(async () => void (await result.current.ask('q')));
    expect(result.current.messages[1]).toMatchObject({ content: 'Part', state: 'failed', error: 'Busy.' });
  });

  it('marks a stream that closes early as failed', async () => {
    fakeEndpoint([new Response(sse('meta', { conversationId: 'C1', userMessageId: 'u', assistantMessageId: 'a', title: 't', model: 'm' }) + sse('delta', { text: 'Part' }))]);
    const { result } = renderHook(() => useDemoChat());
    await act(async () => void (await result.current.ask('q')));
    expect(result.current.messages[1].state).toBe('failed');
  });

  it('ignores a second question while one is streaming', async () => {
    let release: (() => void) | undefined;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const bodies = fakeEndpoint([
      () => {
        throw new Error('unused');
      },
    ]);
    // A response that waits: the fetch promise itself is held.
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_p: string, init: RequestInit) => {
        bodies.push(JSON.parse(new TextDecoder().decode(init.body as Uint8Array)));
        await gate;
        return new Response(answer('C1', 'Done'));
      }),
    );
    const { result } = renderHook(() => useDemoChat());
    let first: Promise<boolean> | undefined;
    act(() => {
      first = result.current.ask('first');
    });
    await act(async () => {
      expect(await result.current.ask('second')).toBe(false);
    });
    release?.();
    await act(async () => void (await first));
    expect(bodies.map((b) => b.message)).toEqual(['first']);
  });
});
