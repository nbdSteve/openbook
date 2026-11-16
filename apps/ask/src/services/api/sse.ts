import type { ChatEvent, Source } from './types';

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

/** Turn a decoded SSE message into a typed chat event. Unknown events return null. */
export function toChatEvent(message: SseMessage): ChatEvent | null {
  let payload: Record<string, unknown>;
  try {
    payload = JSON.parse(message.data) as Record<string, unknown>;
  } catch {
    return { type: 'error', code: 'internal', message: 'The server sent an event the app could not read.' };
  }
  switch (message.event) {
    case 'meta':
      return {
        type: 'meta',
        conversationId: String(payload.conversationId),
        userMessageId: String(payload.userMessageId),
        assistantMessageId: String(payload.assistantMessageId),
        title: String(payload.title ?? ''),
        model: String(payload.model ?? ''),
      };
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
        if (event) yield event;
      }
    }
    for (const message of parser.push(decoder.decode())) {
      const event = toChatEvent(message);
      if (event) yield event;
    }
  } finally {
    reader.releaseLock();
  }
}
