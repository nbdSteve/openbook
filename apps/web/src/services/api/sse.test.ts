import { describe, expect, it } from 'vitest';
import { readChatEvents, SseParser, toChatEvent } from './sse';
import type { ChatEvent } from './types';

const enc = new TextEncoder();

function streamOf(...chunks: string[]): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(enc.encode(chunk));
      controller.close();
    },
  });
}

async function collect(stream: ReadableStream<Uint8Array>): Promise<ChatEvent[]> {
  const out: ChatEvent[] = [];
  for await (const event of readChatEvents(stream)) out.push(event);
  return out;
}

describe('SseParser', () => {
  it('parses several events in one chunk', () => {
    const parser = new SseParser();
    const messages = parser.push('event: delta\ndata: {"text":"a"}\n\nevent: delta\ndata: {"text":"b"}\n\n');
    expect(messages).toEqual([
      { event: 'delta', data: '{"text":"a"}' },
      { event: 'delta', data: '{"text":"b"}' },
    ]);
  });

  it('handles lines that split across chunks, even inside a field name', () => {
    const parser = new SseParser();
    const all = [
      ...parser.push('eve'),
      ...parser.push('nt: del'),
      ...parser.push('ta\ndata: {"te'),
      ...parser.push('xt":"hi"}\n'),
      ...parser.push('\n'),
    ];
    expect(all).toEqual([{ event: 'delta', data: '{"text":"hi"}' }]);
  });

  it('ignores comment lines, including keep-alives between events', () => {
    const parser = new SseParser();
    const all = parser.push(': keep-alive\n\nevent: delta\n: note\ndata: {"text":"x"}\n\n: keep-alive\n\n');
    expect(all).toEqual([{ event: 'delta', data: '{"text":"x"}' }]);
  });

  it('accepts CRLF line ends, even when CR and LF arrive in different chunks', () => {
    const parser = new SseParser();
    const all = [...parser.push('event: done\r\ndata: {}\r'), ...parser.push('\n\r\n')];
    expect(all).toEqual([{ event: 'done', data: '{}' }]);
  });

  it('joins several data lines and keeps leading spaces after the first', () => {
    const parser = new SseParser();
    expect(parser.push('data: a\ndata:  b\n\n')).toEqual([{ event: 'message', data: 'a\n b' }]);
  });

  it('does not emit an event that has no blank line yet', () => {
    const parser = new SseParser();
    expect(parser.push('event: delta\ndata: {"text":"x"}\n')).toEqual([]);
  });
});

describe('readChatEvents', () => {
  it('yields typed events in order for a whole answer', async () => {
    const events = await collect(
      streamOf(
        'event: meta\ndata: {"conversationId":"C1","userMessageId":"U1","assistantMessageId":"A1","title":"Hi","model":"claude-sonnet"}\n\n',
        'event: sources\ndata: {"sources":[{"n":1,"documentId":"D","documentName":"a.pdf","snippet":"s"}]}\n\n',
        'event: delta\ndata: {"text":"Hel"}\n\nevent: delta\ndata: {"text":"lo"}\n\n',
        'event: done\ndata: {"usage":{"inputTokens":3,"outputTokens":2},"stopReason":"end_turn"}\n\n',
      ),
    );
    expect(events.map((e) => e.type)).toEqual(['meta', 'sources', 'delta', 'delta', 'done']);
    expect(events[0]).toMatchObject({ conversationId: 'C1', model: 'claude-sonnet' });
    expect(events[4]).toMatchObject({ stopReason: 'end_turn', usage: { outputTokens: 2 } });
  });

  it('decodes multi-byte characters that split across chunks', async () => {
    const bytes = enc.encode('event: delta\ndata: {"text":"π≈3.14"}\n\n');
    const cut = bytes.indexOf(0xcf) + 1; // inside the two-byte "π"
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(bytes.slice(0, cut));
        controller.enqueue(bytes.slice(cut));
        controller.close();
      },
    });
    expect(await collect(stream)).toEqual([{ type: 'delta', text: 'π≈3.14' }]);
  });

  it('yields the error event that ends a failed stream', async () => {
    const events = await collect(
      streamOf(
        'event: delta\ndata: {"text":"partial"}\n\n',
        'event: error\ndata: {"code":"model_unavailable","message":"The model is busy"}\n\n',
      ),
    );
    expect(events[1]).toEqual({ type: 'error', code: 'model_unavailable', message: 'The model is busy' });
  });

  it('turns unreadable data into an error event instead of throwing', () => {
    expect(toChatEvent({ event: 'delta', data: 'not json' })).toMatchObject({ type: 'error' });
  });

  it('ignores event names it does not know, even with data that is not JSON', () => {
    expect(toChatEvent({ event: 'future', data: '{}' })).toBeNull();
    expect(toChatEvent({ event: 'ping', data: 'plain text' })).toBeNull();
  });

  it('survives data that is JSON but not an object', () => {
    expect(toChatEvent({ event: 'delta', data: 'null' })).toEqual({ type: 'delta', text: '' });
  });

  it('stops reading after the done event', async () => {
    const events = await collect(
      streamOf(
        'event: done\ndata: {"usage":{"inputTokens":1,"outputTokens":1},"stopReason":"end_turn"}\n\n',
        'event: delta\ndata: {"text":"late"}\n\n',
      ),
    );
    expect(events.map((e) => e.type)).toEqual(['done']);
  });
});
