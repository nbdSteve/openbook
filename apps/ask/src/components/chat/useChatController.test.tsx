import { StrictMode, type ReactNode } from 'react';
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const routes = vi.hoisted(() => ({ getConversation: vi.fn(), openChat: vi.fn() }));
vi.mock('@/services/api/routes', () => routes);

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

function setup(conversationId: string | undefined) {
  const onConversationCreated = vi.fn();
  const hook = renderHook(
    ({ id }: { id: string | undefined }) =>
      useChatController({ courseId: 'K', conversationId: id, courseModel: 'claude-sonnet', limits, onConversationCreated, onActivity: vi.fn() }),
    { wrapper, initialProps: { id: conversationId } },
  );
  return { ...hook, onConversationCreated };
}

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
});
