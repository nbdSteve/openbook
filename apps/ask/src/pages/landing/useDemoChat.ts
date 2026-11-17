import { useCallback, useEffect, useRef, useState } from 'react';
import type { ChatMessage } from '@/components/chat/useChatController';
import type { Source } from '@/services/api/types';
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
  ask: (text: string) => Promise<boolean>;
  stop: () => void;
  reset: () => void;
}

let counter = 0;
const localId = (prefix: string) => `${prefix}-${Date.now()}-${counter++}`;

/**
 * The state of the landing page demo: a small version of the course chat. It keeps the
 * conversation ID from the `meta` event so follow-ups work. After the turn limit or the age limit,
 * or when the server no longer knows the conversation, it starts a new one and says so.
 */
export function useDemoChat(): DemoChat {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [status, setStatus] = useState<DemoChat['status']>('idle');
  const [problem, setProblem] = useState<DemoProblem | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const conversation = useRef<{ id: string; turns: number; startedAt: number } | null>(null);
  const abort = useRef<AbortController | null>(null);

  useEffect(() => () => abort.current?.abort(), []);

  const ask = useCallback(
    async (text: string): Promise<boolean> => {
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
      const patch = (id: string, change: (m: ChatMessage) => ChatMessage) =>
        setMessages((all) => all.map((m) => (m.id === id ? change(m) : m)));

      setProblem(null);
      setNotice(fresh ? 'That conversation was full, so this is a fresh one.' : null);
      setStatus('streaming');
      // A new conversation starts with an empty page.
      setMessages((all) => [
        ...(current ? all : []),
        { id: userId, role: 'user', content: message, createdAt: now },
        { id: assistantId, role: 'assistant', content: '', createdAt: now, state: 'streaming' },
      ]);

      const run = async (conversationId: string | undefined): Promise<boolean> => {
        let began = false;
        let finished = false;
        for await (const event of askDemo({ message, conversationId, signal: controller.signal })) {
          began = true;
          switch (event.type) {
            case 'meta':
              {
                const previous = conversationId !== undefined ? current : null;
                conversation.current = {
                  id: event.conversationId,
                  turns: (previous ? previous.turns : 0) + 1,
                  startedAt: previous ? previous.startedAt : Date.now(),
                };
              }
              break;
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
        if (!finished && began) patch(assistantId, (m) => ({ ...m, state: 'failed', error: 'The connection closed before the answer was complete.' }));
        return true;
      };

      try {
        try {
          return await run(current?.id);
        } catch (error) {
          // The server forgot the conversation: start a new one once, and say so.
          if (error instanceof DemoError && error.kind === 'gone' && current) {
            conversation.current = null;
            setMessages([
              { id: userId, role: 'user', content: message, createdAt: now },
              { id: assistantId, role: 'assistant', content: '', createdAt: now, state: 'streaming' },
            ]);
            setNotice('That conversation ended, so this is a fresh one.');
            current = null;
            return await run(undefined);
          }
          throw error;
        }
      } catch (error) {
        if (controller.signal.aborted) {
          patch(assistantId, (m) => ({ ...m, state: 'stopped' }));
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
    },
    [],
  );

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

  return { messages, status, problem, notice, ask, stop, reset };
}
