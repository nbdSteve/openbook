import { useCallback, useEffect, useRef, useState } from 'react';
import { describeApiError } from '@/services/api/errors';
import { getConversation, openChat } from '@/services/api/routes';
import { readChatEvents } from '@/services/api/sse';
import type { Limits, Message, Source, TutoringStyle } from '@/services/api/types';

export interface ChatMessage extends Message {
  state?: 'streaming' | 'stopped' | 'failed';
  error?: string;
}

export interface ChatController {
  messages: ChatMessage[];
  status: 'idle' | 'loading' | 'streaming';
  loadError: string | null;
  sendError: string | null;
  model: string;
  setModel: (id: string) => void;
  style: TutoringStyle;
  setStyle: (style: TutoringStyle) => void;
  styleLocked: boolean;
  send: (text: string) => Promise<boolean>;
  stop: () => void;
  dismissSendError: () => void;
}

interface Options {
  courseId: string;
  conversationId: string | undefined;
  courseModel: string;
  courseStyle: TutoringStyle;
  allowStyleChoice: boolean;
  limits: Limits;
  onConversationCreated: (conversationId: string) => void;
  onActivity: () => void;
}

let localCounter = 0;
const localId = (prefix: string) => `${prefix}-${Date.now()}-${localCounter++}`;

export function useChatController({
  courseId,
  conversationId,
  courseModel,
  courseStyle,
  allowStyleChoice,
  limits,
  onConversationCreated,
  onActivity,
}: Options): ChatController {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [status, setStatus] = useState<ChatController['status']>(conversationId ? 'loading' : 'idle');
  const [loadError, setLoadError] = useState<string | null>(null);
  const [sendError, setSendError] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [conversationModel, setConversationModel] = useState<string | undefined>(undefined);
  const [pickedStyle, setPickedStyle] = useState<TutoringStyle | null>(null);
  const [conversationStyle, setConversationStyle] = useState<TutoringStyle | undefined>(undefined);

  const shownId = useRef<string | undefined>(undefined);
  const createdId = useRef<string | undefined>(undefined);
  const streamAbort = useRef<AbortController | null>(null);

  useEffect(() => {
    if (conversationId && conversationId === createdId.current) return;
    createdId.current = undefined;
    streamAbort.current?.abort();
    streamAbort.current = null;
    shownId.current = conversationId;
    const controller = new AbortController();
    setMessages([]);
    setLoadError(null);
    setSendError(null);
    setPicked(null);
    setConversationModel(undefined);
    setPickedStyle(null);
    setConversationStyle(undefined);
    if (!conversationId) {
      setStatus('idle');
      return;
    }
    setStatus('loading');
    getConversation(conversationId, controller.signal).then(
      (result) => {
        if (controller.signal.aborted) return;
        setMessages(result.messages);
        setConversationModel(result.conversation.model);
        setConversationStyle(result.conversation.style);
        setStatus('idle');
      },
      (error: unknown) => {
        if (controller.signal.aborted) return;
        setLoadError(describeApiError(error));
        setStatus('idle');
      },
    );
    return () => controller.abort();
  }, [conversationId]);

  useEffect(() => () => streamAbort.current?.abort(), []);

  const send = useCallback(
    async (text: string): Promise<boolean> => {
      const message = text.trim();
      if (!message) return false;
      const currentModel = picked ?? conversationModel ?? courseModel;
      const currentStyle = allowStyleChoice ? (pickedStyle ?? conversationStyle ?? courseStyle) : undefined;

      streamAbort.current?.abort();
      const controller = new AbortController();
      streamAbort.current = controller;

      const userId = localId('user');
      const assistantId = localId('assistant');
      const now = new Date().toISOString();
      const patch = (id: string, change: (m: ChatMessage) => ChatMessage) =>
        setMessages((all) => all.map((m) => (m.id === id ? change(m) : m)));

      setSendError(null);
      setStatus('streaming');
      setMessages((all) => [
        ...all,
        { id: userId, role: 'user', content: message, createdAt: now },
        { id: assistantId, role: 'assistant', content: '', createdAt: now, state: 'streaming' },
      ]);

      let began = false;
      let finished = false;
      try {
        const body = await openChat(courseId, { conversationId: shownId.current, message, model: currentModel, ...(currentStyle ? { style: currentStyle } : {}) }, controller.signal);
        began = true;
        for await (const event of readChatEvents(body)) {
          switch (event.type) {
            case 'meta':
              if (!shownId.current) {
                shownId.current = event.conversationId;
                createdId.current = event.conversationId;
                onConversationCreated(event.conversationId);
              }
              setConversationModel(event.model || undefined);
              setPicked(null);
              if (event.style) setConversationStyle(event.style);
              setPickedStyle(null);
              patch(assistantId, (m) => ({ ...m, model: event.model || undefined }));
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
        if (!finished) {
          patch(assistantId, (m) => ({
            ...m,
            state: 'failed',
            error: 'The connection closed before the answer was complete.',
          }));
        }
        return true;
      } catch (error) {
        if (controller.signal.aborted) {
          patch(assistantId, (m) => ({ ...m, state: 'stopped' }));
          return true;
        }
        if (!began) {
          setMessages((all) => all.filter((m) => m.id !== userId && m.id !== assistantId));
          setSendError(describeApiError(error, limits));
          return false;
        }
        patch(assistantId, (m) => ({ ...m, state: 'failed', error: describeApiError(error, limits) }));
        return true;
      } finally {
        if (streamAbort.current === controller) {
          setStatus('idle');
          streamAbort.current = null;
        }
        onActivity();
      }
    },
    [courseId, picked, conversationModel, courseModel, pickedStyle, conversationStyle, courseStyle, allowStyleChoice, limits, onConversationCreated, onActivity],
  );

  const stop = useCallback(() => streamAbort.current?.abort(), []);
  const dismissSendError = useCallback(() => setSendError(null), []);

  const model = picked ?? conversationModel ?? courseModel;
  const style = allowStyleChoice ? (pickedStyle ?? conversationStyle ?? courseStyle) : courseStyle;
  return {
    messages,
    status,
    loadError,
    sendError,
    model,
    setModel: setPicked,
    style,
    setStyle: setPickedStyle,
    styleLocked: !allowStyleChoice,
    send,
    stop,
    dismissSendError,
  };
}
