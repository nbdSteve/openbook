import { useCallback, useEffect, useRef, useState } from 'react';
import { describeApiError } from '@/services/api/errors';
import { getConversation, openChat } from '@/services/api/routes';
import { readChatEvents } from '@/services/api/sse';
import type { Limits, Message, Source } from '@/services/api/types';

export interface ChatMessage extends Message {
  /** Set only on assistant messages that are not complete. */
  state?: 'streaming' | 'stopped' | 'failed';
  error?: string;
}

export interface ChatController {
  messages: ChatMessage[];
  /** `loading` while an existing conversation loads, `streaming` while an answer arrives. */
  status: 'idle' | 'loading' | 'streaming';
  /** Failure to load the conversation. */
  loadError: string | null;
  /** Failure that happened before the stream began (quota, capacity, and so on). */
  sendError: string | null;
  /** The catalog ID the next message uses: the picked model, else the conversation's, else the course default. */
  model: string;
  setModel: (id: string) => void;
  /** Returns false when the server refused the message before the stream began. */
  send: (text: string) => Promise<boolean>;
  stop: () => void;
  dismissSendError: () => void;
}

interface Options {
  courseId: string;
  /** The conversation in the URL, or undefined for a new chat. */
  conversationId: string | undefined;
  /** The course default model. */
  courseModel: string;
  limits: Limits;
  onConversationCreated: (conversationId: string) => void;
  onActivity: () => void;
}

let localCounter = 0;
const localId = (prefix: string) => `${prefix}-${Date.now()}-${localCounter++}`;

/**
 * Owns the messages of the open conversation and the streaming answer. It lives above the routes,
 * so a stream survives the switch from "new chat" to the new conversation's URL.
 */
export function useChatController({ courseId, conversationId, courseModel, limits, onConversationCreated, onActivity }: Options): ChatController {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [status, setStatus] = useState<ChatController['status']>(conversationId ? 'loading' : 'idle');
  const [loadError, setLoadError] = useState<string | null>(null);
  const [sendError, setSendError] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [conversationModel, setConversationModel] = useState<string | undefined>(undefined);

  // The conversation that `messages` shows. A new chat has none until the `meta` event.
  const shownId = useRef<string | undefined>(undefined);
  // The conversation that a live stream created. Its URL changes to it, but there is nothing to load.
  const createdId = useRef<string | undefined>(undefined);
  const streamAbort = useRef<AbortController | null>(null);

  useEffect(() => {
    if (conversationId && conversationId === createdId.current) return;
    createdId.current = undefined;
    // Leaving a conversation stops its answer. The server still saves it.
    streamAbort.current?.abort();
    streamAbort.current = null;
    shownId.current = conversationId;
    // This load has its own controller, so React StrictMode (effect, cleanup, effect) loads again.
    const controller = new AbortController();
    // A route change resets the chat.
    setMessages([]);
    setLoadError(null);
    setSendError(null);
    setPicked(null);
    setConversationModel(undefined);
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
      // Always send the model that the picker shows. After a failed answer the server may still hold the
      // conversation's old model, so leaving it out could use a different model than the one on screen.
      const currentModel = picked ?? conversationModel ?? courseModel;

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
        const body = await openChat(courseId, { conversationId: shownId.current, message, model: currentModel }, controller.signal);
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
          // The server refused before it streamed anything: take the pending messages back.
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
    [courseId, picked, conversationModel, courseModel, limits, onConversationCreated, onActivity],
  );

  const stop = useCallback(() => streamAbort.current?.abort(), []);
  const dismissSendError = useCallback(() => setSendError(null), []);

  const model = picked ?? conversationModel ?? courseModel;
  return { messages, status, loadError, sendError, model, setModel: setPicked, send, stop, dismissSendError };
}
