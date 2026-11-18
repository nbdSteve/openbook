import { StrictMode, type ReactNode } from 'react';
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const routes = vi.hoisted(() => ({ getConversation: vi.fn(), openChat: vi.fn() }));
vi.mock('@/services/api/routes', () => routes);

import type { TutoringStyle } from '@/services/api/types';
import { useChatController } from './useChatController';

const limits = { dailyMessages: 50, maxCoursesOwned: 10, maxDocumentsPerCourse: 50, maxUploadBytes: 1000 };
const wrapper = ({ children }: { children: ReactNode }) => <StrictMode>{children}</StrictMode>;

function sse(...events: string[]): ReadableStream<Uint8Array> {
  const enc = new TextEncoder();
  return new ReadableStream({
    start(controller) {
      for (const e of events) controller.enqueue(enc.encode(e));
      controller.close();
    },
  });
}

const stored = {
  conversation: { id: 'C1', courseId: 'K', title: 't', model: 'claude-sonnet', createdAt: '', updatedAt: '' },
  messages: [{ id: 'M1', role: 'user', content: 'hello', createdAt: '' }],
};

beforeEach(() => {
  routes.getConversation.mockReset();
  routes.openChat.mockReset();
});

interface SetupOptions {
  courseStyle?: TutoringStyle;
  allowStyleChoice?: boolean;
}

function setup(conversationId: string | undefined, { courseStyle = 'socratic', allowStyleChoice = true }: SetupOptions = {}) {
  const onConversationCreated = vi.fn();
  const hook = renderHook(
    ({ id }: { id: string | undefined }) =>
      useChatController({
        courseId: 'K',
        conversationId: id,
        courseModel: 'claude-sonnet',
        courseStyle,
        allowStyleChoice,
        limits,
        onConversationCreated,
        onActivity: vi.fn(),
      }),
    { wrapper, initialProps: { id: conversationId } },
  );
  return { ...hook, onConversationCreated };
}

const DONE = 'event: done\ndata: {"usage":{"inputTokens":1,"outputTokens":1},"stopReason":"end_turn"}\n\n';
const meta = (style: string) =>
  `event: meta\ndata: {"conversationId":"C9","userMessageId":"U","assistantMessageId":"A","title":"t","model":"claude-sonnet","style":"${style}"}\n\n`;

describe('useChatController', () => {
  it('loads an existing conversation even under StrictMode (effect, cleanup, effect)', async () => {
    routes.getConversation.mockResolvedValue(stored);
    const { result } = setup('C1');
    await waitFor(() => expect(result.current.messages).toHaveLength(1));
    expect(result.current.status).toBe('idle');
    expect(result.current.model).toBe('claude-sonnet');
  });

  it('does not reload the conversation that its own stream just created', async () => {
    routes.openChat.mockResolvedValue(
      sse(
        'event: meta\ndata: {"conversationId":"C9","userMessageId":"U","assistantMessageId":"A","title":"t","model":"claude-sonnet"}\n\n',
        'event: delta\ndata: {"text":"Hi"}\n\n',
        'event: done\ndata: {"usage":{"inputTokens":1,"outputTokens":1},"stopReason":"end_turn"}\n\n',
      ),
    );
    const { result, rerender, onConversationCreated } = setup(undefined);
    await act(async () => {
      await result.current.send('hello');
    });
    expect(onConversationCreated).toHaveBeenCalledWith('C9');
    rerender({ id: 'C9' });
    expect(routes.getConversation).not.toHaveBeenCalled();
    expect(result.current.messages.map((m) => m.content)).toEqual(['hello', 'Hi']);
    expect(result.current.messages[1].state).toBeUndefined();
  });

  it('takes back the pending messages when the server refuses before the stream starts', async () => {
    const { ApiError } = await import('@/services/api/client');
    routes.openChat.mockRejectedValue(new ApiError(429, 'quota_exceeded', 'No more'));
    const { result } = setup(undefined);
    let accepted = true;
    await act(async () => {
      accepted = await result.current.send('hello');
    });
    expect(accepted).toBe(false);
    expect(result.current.messages).toEqual([]);
    expect(result.current.sendError).toMatch(/all 50 messages/);
  });

  it('marks a stream that ends without done as failed', async () => {
    routes.openChat.mockResolvedValue(sse('event: delta\ndata: {"text":"partial"}\n\n'));
    const { result } = setup(undefined);
    await act(async () => {
      await result.current.send('hello');
    });
    expect(result.current.messages[1]).toMatchObject({ content: 'partial', state: 'failed' });
  });

  it('sends the model the picker shows, also after an answer failed', async () => {
    routes.openChat
      .mockResolvedValueOnce(
        sse(
          'event: meta\ndata: {"conversationId":"C9","userMessageId":"U","assistantMessageId":"A","title":"t","model":"claude-haiku"}\n\n',
          'event: error\ndata: {"code":"model_unavailable","message":"busy"}\n\n',
        ),
      )
      .mockResolvedValueOnce(sse('event: done\ndata: {"usage":{"inputTokens":1,"outputTokens":1},"stopReason":"end_turn"}\n\n'));
    const { result } = setup(undefined);
    act(() => result.current.setModel('claude-haiku'));
    await act(async () => {
      await result.current.send('first');
    });
    expect(result.current.messages[1].state).toBe('failed');
    // The picker shows the model of the failed answer. The next send must name it.
    expect(result.current.model).toBe('claude-haiku');
    await act(async () => {
      await result.current.send('second');
    });
    expect(routes.openChat).toHaveBeenNthCalledWith(1, 'K', expect.objectContaining({ model: 'claude-haiku' }), expect.anything());
    expect(routes.openChat).toHaveBeenNthCalledWith(2, 'K', expect.objectContaining({ model: 'claude-haiku' }), expect.anything());

    act(() => result.current.setModel('claude-sonnet'));
    await act(async () => {
      await result.current.send('third');
    });
    expect(routes.openChat).toHaveBeenNthCalledWith(3, 'K', expect.objectContaining({ model: 'claude-sonnet' }), expect.anything());
  });

  describe('the tutoring style', () => {
    it('starts with the course default and sends it', async () => {
      routes.openChat.mockResolvedValue(sse(meta('hints'), DONE));
      const { result } = setup(undefined, { courseStyle: 'hints' });
      expect(result.current.style).toBe('hints');
      expect(result.current.styleLocked).toBe(false);
      await act(async () => {
        await result.current.send('hello');
      });
      expect(routes.openChat).toHaveBeenCalledWith('K', expect.objectContaining({ style: 'hints' }), expect.anything());
    });

    it('applies a pick from the next message and keeps it for the follow-ups', async () => {
      routes.openChat.mockResolvedValueOnce(sse(meta('direct'), DONE)).mockResolvedValueOnce(sse(meta('direct'), DONE));
      const { result } = setup(undefined);
      act(() => result.current.setStyle('direct'));
      expect(result.current.style).toBe('direct');
      await act(async () => {
        await result.current.send('first');
      });
      expect(result.current.style).toBe('direct');
      await act(async () => {
        await result.current.send('second');
      });
      expect(routes.openChat).toHaveBeenNthCalledWith(1, 'K', expect.objectContaining({ style: 'direct' }), expect.anything());
      expect(routes.openChat).toHaveBeenNthCalledWith(2, 'K', expect.objectContaining({ style: 'direct' }), expect.anything());
    });

    it('changes the style in the middle of a conversation', async () => {
      routes.openChat.mockResolvedValueOnce(sse(meta('socratic'), DONE)).mockResolvedValueOnce(sse(meta('concepts'), DONE));
      const { result } = setup(undefined);
      await act(async () => {
        await result.current.send('first');
      });
      expect(result.current.style).toBe('socratic');
      act(() => result.current.setStyle('concepts'));
      expect(result.current.style).toBe('concepts');
      await act(async () => {
        await result.current.send('second');
      });
      expect(routes.openChat).toHaveBeenNthCalledWith(2, 'K', expect.objectContaining({ conversationId: 'C9', style: 'concepts' }), expect.anything());
      expect(result.current.style).toBe('concepts');
    });

    it('shows the style of a stored conversation, and a new chat goes back to the course default', async () => {
      routes.getConversation.mockResolvedValue({ ...stored, conversation: { ...stored.conversation, style: 'step_by_step' } });
      const { result, rerender } = setup('C1');
      await waitFor(() => expect(result.current.messages).toHaveLength(1));
      expect(result.current.style).toBe('step_by_step');
      rerender({ id: undefined });
      await waitFor(() => expect(result.current.messages).toHaveLength(0));
      expect(result.current.style).toBe('socratic');
    });

    it('uses the course style, and sends none, when the course is locked', async () => {
      routes.getConversation.mockResolvedValue({ ...stored, conversation: { ...stored.conversation, style: 'direct' } });
      routes.openChat.mockResolvedValue(sse(meta('hints'), DONE));
      const { result } = setup('C1', { courseStyle: 'hints', allowStyleChoice: false });
      await waitFor(() => expect(result.current.messages).toHaveLength(1));
      expect(result.current.styleLocked).toBe(true);
      expect(result.current.style).toBe('hints');
      act(() => result.current.setStyle('direct'));
      expect(result.current.style).toBe('hints');
      await act(async () => {
        await result.current.send('hello');
      });
      expect(routes.openChat.mock.calls[0][1]).not.toHaveProperty('style');
    });
  });
});
