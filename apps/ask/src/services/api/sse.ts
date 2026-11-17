import type { Attribution, ChatEvent, Source } from './types';

export interface SseMessage {
  event: string;
  data: string;
}

/**
 * Incremental parser for Server-Sent Events. Feed it text chunks of any size. Lines can split
 * across chunks. It handles LF, CRLF, and CR line ends, ignores `:` comment lines, joins several
 * `data:` lines with a newline, and drops an event that has no blank-line terminator yet.
 */
export class SseParser {
  private buffer = '';
  private event = '';
  private data: string[] = [];
  private sawData = false;
  private lastWasCr = false;

  push(chunk: string): SseMessage[] {
    const out: SseMessage[] = [];
    let text = chunk;
    // A CRLF pair can split across chunks: drop the LF that follows a CR we already handled.
    if (this.lastWasCr && text.startsWith('\n')) text = text.slice(1);
    this.lastWasCr = text.endsWith('\r');
    this.buffer += text;

    let start = 0;
    for (let i = 0; i < this.buffer.length; i++) {
      const ch = this.buffer[i];
      if (ch !== '\n' && ch !== '\r') continue;
      const line = this.buffer.slice(start, i);
      if (ch === '\r' && this.buffer[i + 1] === '\n') i++;
      start = i + 1;
      const message = this.line(line);
      if (message) out.push(message);
    }
    this.buffer = this.buffer.slice(start);
    return out;
  }

  private line(line: string): SseMessage | null {
    if (line === '') {
      if (!this.sawData) {
        this.event = '';
        return null;
      }
      const message = { event: this.event || 'message', data: this.data.join('\n') };
      this.event = '';
      this.data = [];
      this.sawData = false;
      return message;
    }
    if (line.startsWith(':')) return null;
    const colon = line.indexOf(':');
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') this.event = value;
    else if (field === 'data') {
      this.data.push(value);
      this.sawData = true;
    }
    return null;
  }
}

/** Keep the credit lines that have the fields the page needs. */
function readAttribution(items: unknown[]): Attribution[] {
  return items.flatMap((item): Attribution[] => {
    const a = item as Partial<Record<keyof Attribution, unknown>> | null;
    if (!a || typeof a.title !== 'string' || typeof a.license !== 'string') return [];
    return [
      {
        title: a.title,
        publisher: typeof a.publisher === 'string' ? a.publisher : '',
        license: a.license,
        ...(typeof a.licenseUrl === 'string' && { licenseUrl: a.licenseUrl }),
        ...(typeof a.sourceUrl === 'string' && { sourceUrl: a.sourceUrl }),
      },
    ];
  });
}

/** Turn a decoded SSE message into a typed chat event. Unknown events return null. */
const KNOWN_EVENTS = new Set(['meta', 'sources', 'delta', 'done', 'error']);

export function toChatEvent(message: SseMessage): ChatEvent | null {
  // An event the app does not know is ignored, whatever its data looks like.
  if (!KNOWN_EVENTS.has(message.event)) return null;
  let payload: Record<string, unknown>;
  try {
    const parsed: unknown = JSON.parse(message.data);
    payload = parsed !== null && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {};
  } catch {
    return { type: 'error', code: 'internal', message: 'The server sent an event the app could not read.' };
  }
  switch (message.event) {
    case 'meta': {
      const text = (value: unknown) => (typeof value === 'string' ? value : undefined);
      return {
        type: 'meta',
        conversationId: String(payload.conversationId),
        model: String(payload.model ?? ''),
        ...(text(payload.userMessageId) !== undefined && { userMessageId: text(payload.userMessageId) }),
        ...(text(payload.assistantMessageId) !== undefined && { assistantMessageId: text(payload.assistantMessageId) }),
        ...(text(payload.title) !== undefined && { title: text(payload.title) }),
        ...(Array.isArray(payload.attribution) && { attribution: readAttribution(payload.attribution) }),
      };
    }
    case 'sources':
      return { type: 'sources', sources: Array.isArray(payload.sources) ? (payload.sources as Source[]) : [] };
    case 'delta':
      return { type: 'delta', text: String(payload.text ?? '') };
    case 'done':
      return {
        type: 'done',
        usage: (payload.usage as { inputTokens: number; outputTokens: number }) ?? { inputTokens: 0, outputTokens: 0 },
        stopReason: String(payload.stopReason ?? ''),
      };
    case 'error':
      return {
        type: 'error',
        code: String(payload.code ?? 'internal'),
        message: String(payload.message ?? 'The answer failed.'),
      };
    default:
      return null;
  }
}

/** Read a byte stream and yield typed chat events. */
export async function* readChatEvents(body: ReadableStream<Uint8Array>): AsyncGenerator<ChatEvent> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const parser = new SseParser();
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      for (const message of parser.push(decoder.decode(value, { stream: true }))) {
        const event = toChatEvent(message);
        if (!event) continue;
        yield event;
        // `done` and `error` end the stream. Ignore anything after them.
        if (event.type === 'done' || event.type === 'error') return;
      }
    }
    for (const message of parser.push(decoder.decode())) {
      const event = toChatEvent(message);
      if (event) {
        yield event;
        if (event.type === 'done' || event.type === 'error') return;
      }
    }
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
