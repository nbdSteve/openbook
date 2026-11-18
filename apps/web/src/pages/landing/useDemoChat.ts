import { useCallback, useEffect, useRef, useState } from 'react';
import { DEMO_DEFAULT_STYLE } from '@/lib/constants';
import type { ChatMessage } from '@/components/chat/useChatController';
import type { Attribution, Source, TutoringStyle } from '@/services/api/types';
import { askDemo, DEMO_MAX_AGE_MS, DEMO_MAX_TURNS, DemoError, describeDemoError, type DemoErrorKind } from '@/services/demo/demoClient';

export interface DemoProblem {
  kind: DemoErrorKind;
  message: string;
}

export interface DemoChat {
  messages: ChatMessage[];
  status: 'idle' | 'streaming';
  problem: DemoProblem | null;
  notice: string | null;
  attribution: Attribution[];
  style: TutoringStyle;
  setStyle: (style: TutoringStyle) => void;
  ask: (text: string) => Promise<boolean>;
  stop: () => void;
  reset: () => void;
}

let counter = 0;
const localId = (prefix: string) => `${prefix}-${Date.now()}-${counter++}`;

export function useDemoChat(): DemoChat {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [status, setStatus] = useState<DemoChat['status']>('idle');
  const [problem, setProblem] = useState<DemoProblem | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [attribution, setAttribution] = useState<Attribution[]>([]);
  const conversation = useRef<{ id: string; turns: number; startedAt: number } | null>(null);
  const abort = useRef<AbortController | null>(null);
  const [picked, setPicked] = useState<TutoringStyle | null>(null);
  const [served, setServed] = useState<TutoringStyle | null>(null);
  const pickedRef = useRef<TutoringStyle | null>(null);

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
    setMessages((all) => [
      ...(current ? all : []),
      { id: userId, role: 'user', content: message, createdAt: now },
      { id: assistantId, role: 'assistant', content: '', createdAt: now, state: 'streaming' },
    ]);

    let began = false;
    let finished = false;
    try {
      for await (const event of askDemo({ message, conversationId: current?.id, style: pickedRef.current ?? undefined, signal: controller.signal })) {
        began = true;
        switch (event.type) {
          case 'meta': {
            const continued = current !== null && event.conversationId === current.id;
            conversation.current = {
              id: event.conversationId,
              turns: (continued && current ? current.turns : 0) + 1,
              startedAt: continued && current ? current.startedAt : Date.now(),
            };
            if (current && !continued) setNotice('That conversation ended, so this is a fresh one.');
            if (event.style) setServed(event.style);
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
    pickedRef.current = null;
    setPicked(null);
    setServed(null);
    setStatus('idle');
  }, []);

  const setStyle = useCallback((style: TutoringStyle) => {
    pickedRef.current = style;
    setPicked(style);
  }, []);

  return { messages, status, problem, notice, attribution, style: picked ?? served ?? DEMO_DEFAULT_STYLE, setStyle, ask, stop, reset };
}
