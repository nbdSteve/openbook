import { useCallback, useEffect, useRef, useState } from 'react';
import type { ChatMessage } from '@/components/chat/useChatController';
import type { Attribution, Source } from '@/services/api/types';
import { askDemo, DEMO_MAX_AGE_MS, DEMO_MAX_TURNS, DemoError, describeDemoError, type DemoErrorKind } from '@/services/demo/demoClient';

export interface DemoProblem {
  kind: DemoErrorKind;
  message: string;
}

export interface DemoChat {
  messages: ChatMessage[];
  status: 'idle' | 'streaming';
  /** A problem that stopped the last question before any answer began. */
  problem: DemoProblem | null;
  /** A friendly note, for example that the conversation was full and a new one began. */
  notice: string | null;
  /** The credit lines of the books, from the last answer. */
  attribution: Attribution[];
  ask: (text: string) => Promise<boolean>;
  stop: () => void;
  reset: () => void;
}

let counter = 0;
const localId = (prefix: string) => `${prefix}-${Date.now()}-${counter++}`;

/**
 * The state of the landing page demo: a small version of the course chat. It keeps the
 * conversation ID from the `meta` event so follow-ups work. After the turn limit or the age limit
 * it starts a new conversation and says so. The server does the same when it no longer knows the
 * ID: the `meta` event then has a new ID, and the notice says so.
 */
export function useDemoChat(): DemoChat {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [status, setStatus] = useState<DemoChat['status']>('idle');
  const [problem, setProblem] = useState<DemoProblem | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [attribution, setAttribution] = useState<Attribution[]>([]);
  const conversation = useRef<{ id: string; turns: number; startedAt: number } | null>(null);
  const abort = useRef<AbortController | null>(null);

  useEffect(() => () => abort.current?.abort(), []);

  const ask = useCallback(async (text: string): Promise<boolean> => {
    const message = text.trim();
    if (!message || abort.current) return false;

    let current = conversation.current;
    let fresh = false;
    if (current && (current.turns >= DEMO_MAX_TURNS || Date.now() - current.startedAt > DEMO_MAX_AGE_MS)) {
      current = null;
      fresh = true;
    }

    const controller = new AbortController();
    abort.current = controller;
    const userId = localId('user');
    const assistantId = localId('assistant');
    const now = new Date().toISOString();
    const patch = (id: string, change: (m: ChatMessage) => ChatMessage) => setMessages((all) => all.map((m) => (m.id === id ? change(m) : m)));

    setProblem(null);
    setNotice(fresh ? 'That conversation was full, so this is a fresh one.' : null);
    setStatus('streaming');
    // A new conversation starts with an empty page.
    setMessages((all) => [
      ...(current ? all : []),
      { id: userId, role: 'user', content: message, createdAt: now },
      { id: assistantId, role: 'assistant', content: '', createdAt: now, state: 'streaming' },
    ]);

    // Like the course chat: once an event has come, the question and the partial answer stay, whatever happens next.
    let began = false;
    let finished = false;
    try {
      for await (const event of askDemo({ message, conversationId: current?.id, signal: controller.signal })) {
        began = true;
        switch (event.type) {
          case 'meta': {
            const continued = current !== null && event.conversationId === current.id;
            conversation.current = {
              id: event.conversationId,
              turns: (continued && current ? current.turns : 0) + 1,
              startedAt: continued && current ? current.startedAt : Date.now(),
            };
            // The server forgot the old conversation (it expired, or it is not known): this is a new one.
            if (current && !continued) setNotice('That conversation ended, so this is a fresh one.');
            if (event.attribution && event.attribution.length > 0) setAttribution(event.attribution);
            break;
          }
          case 'sources':
            patch(assistantId, (m) => ({ ...m, sources: event.sources as Source[] }));
            break;
          case 'delta':
            patch(assistantId, (m) => ({ ...m, content: m.content + event.text }));
            break;
          case 'done':
            finished = true;
            patch(assistantId, (m) => ({ ...m, state: undefined }));
            break;
          case 'error':
            finished = true;
            patch(assistantId, (m) => ({ ...m, state: 'failed', error: event.message }));
            break;
        }
      }
      if (!finished) {
        // The stream ended with no `done`, or with no event at all: never leave the bubble thinking.
        patch(assistantId, (m) => ({ ...m, state: 'failed', error: 'The connection closed before the answer was complete.' }));
      }
      return true;
    } catch (error) {
      if (controller.signal.aborted) {
        patch(assistantId, (m) => ({ ...m, state: 'stopped' }));
        return true;
      }
      if (began) {
        patch(assistantId, (m) => ({
          ...m,
          state: 'failed',
          error: error instanceof DemoError ? describeDemoError(error) : 'The connection closed before the answer was complete.',
        }));
        return true;
      }
      // Nothing streamed: take the pending messages back and show the problem under the box.
      setMessages((all) => all.filter((m) => m.id !== userId && m.id !== assistantId));
      setProblem({ kind: error instanceof DemoError ? error.kind : 'other', message: describeDemoError(error) });
      setNotice(null);
      return false;
    } finally {
      if (abort.current === controller) abort.current = null;
      setStatus('idle');
    }
  }, []);

  const stop = useCallback(() => abort.current?.abort(), []);
  const reset = useCallback(() => {
    abort.current?.abort();
    abort.current = null;
    conversation.current = null;
    setMessages([]);
    setProblem(null);
    setNotice(null);
    setStatus('idle');
  }, []);

  return { messages, status, problem, notice, attribution, ask, stop, reset };
}
