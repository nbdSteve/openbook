import { sha256Hex } from '@/services/api/client';
import { readChatEvents } from '@/services/api/sse';
import type { ChatEvent, TutoringStyle } from '@/services/api/types';

/**
 * The client of the public demo endpoint. This is the only module that knows its name and its
 * error codes, so a rename is one change. The endpoint needs no account. A `conversationId` from
 * the `meta` event lets a visitor ask follow-ups.
 */
export const DEMO_PATH = '/api/demo/chat';

/** The longest question the endpoint takes. */
export const DEMO_MAX_QUESTION = 300;
/** A demo conversation lasts this many turns and this many hours. The client starts a new one after that. */
export const DEMO_MAX_TURNS = 10;
export const DEMO_MAX_AGE_MS = 24 * 60 * 60 * 1000;

export type DemoErrorKind = 'rate_limited' | 'busy' | 'unavailable' | 'invalid' | 'network' | 'other';

export class DemoError extends Error {
  readonly kind: DemoErrorKind;
  readonly retryAfterSeconds?: number;

  constructor(kind: DemoErrorKind, message: string, retryAfterSeconds?: number) {
    super(message);
    this.name = 'DemoError';
    this.kind = kind;
    this.retryAfterSeconds = retryAfterSeconds;
  }
}

/** The demo route sets this header on every response it makes. A response without it came from somewhere else. */
const DEMO_HEADER = 'x-openbook-demo';

function kindOf(status: number, code: string, fromDemo: boolean): DemoErrorKind {
  // Lambda's own 429 (a throttle), or a 502 or 504 page of CloudFront: the demo function did not answer, so it is busy.
  if (!fromDemo && (status === 429 || status >= 500)) return 'busy';
  if (code === 'demo_rate_limited' || status === 429) return 'rate_limited';
  if (code === 'demo_busy') return 'busy';
  if (code === 'demo_unavailable') return 'unavailable';
  // A CloudFront function rewrites a Lambda throttle or timeout to 503. When the body is not our JSON
  // error, the demo is busy (docs/api.md, "Demo").
  if (status === 503) return code ? 'unavailable' : 'busy';
  if (code === 'invalid_request' || status === 400) return 'invalid';
  return 'other';
}

async function toDemoError(response: Response): Promise<DemoError> {
  let code = '';
  let message = '';
  let retry: number | undefined;
  try {
    const body = (await response.json()) as { error?: { code?: unknown; message?: unknown; retryAfterSeconds?: unknown } };
    if (typeof body.error?.code === 'string') code = body.error.code;
    if (typeof body.error?.message === 'string') message = body.error.message;
    if (typeof body.error?.retryAfterSeconds === 'number') retry = body.error.retryAfterSeconds;
  } catch {
    // Not JSON. The status decides.
  }
  if (retry === undefined) {
    const header = Number(response.headers.get('retry-after'));
    if (Number.isFinite(header) && header > 0) retry = header;
  }
  return new DemoError(kindOf(response.status, code, response.headers.has(DEMO_HEADER)), message || `The demo failed with status ${response.status}`, retry);
}

/**
 * Ask one question. It yields the same events as the course chat. It throws DemoError before the
 * stream starts (limits, validation) and for a network failure.
 */
export async function* askDemo(options: {
  message: string;
  conversationId?: string;
  style?: TutoringStyle;
  signal?: AbortSignal;
}): AsyncGenerator<ChatEvent> {
  const body = new TextEncoder().encode(
    JSON.stringify({
      message: options.message,
      ...(options.conversationId ? { conversationId: options.conversationId } : {}),
      ...(options.style ? { style: options.style } : {}),
    }),
  );
  let response: Response;
  try {
    response = await fetch(DEMO_PATH, {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-amz-content-sha256': await sha256Hex(body) },
      body,
      signal: options.signal,
    });
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') throw error;
    throw new DemoError('network', 'Could not reach the demo.');
  }
  if (!response.ok) throw await toDemoError(response);
  if (!response.body) throw new DemoError('other', 'The demo sent no answer.');
  yield* readChatEvents(response.body);
}

/** A friendly sentence for a visitor. */
export function describeDemoError(error: unknown): string {
  if (!(error instanceof DemoError)) return 'The demo had a problem. Try again in a moment.';
  switch (error.kind) {
    case 'rate_limited': {
      const s = error.retryAfterSeconds;
      if (!s) return 'You are asking fast. Give it a minute, then try again.';
      return s >= 90 ? `You are asking fast. Try again in ${Math.ceil(s / 60)} minutes.` : `You are asking fast. Try again in ${s} seconds.`;
    }
    case 'busy':
      return 'The demo has helped enough people for today. Come back tomorrow, or create an account.';
    case 'unavailable':
      return 'The demo is resting. Try again later, or create an account.';
    case 'invalid':
      return error.message || `Keep the question under ${DEMO_MAX_QUESTION} characters.`;
    case 'network':
      return 'Could not reach the demo. Check your connection and try again.';
    default:
      return 'The demo had a problem. Try again in a moment.';
  }
}
