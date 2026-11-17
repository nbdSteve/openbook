import { useMemo } from 'react';
import { DEMO_CAPTIONS } from '@/components/chat/captions';
import { MessageBubble } from '@/components/chat/MessageBubble';
import type { ChatMessage } from '@/components/chat/useChatController';
import { ModelsContext } from '@/services/models/ModelsContext';

/**
 * The demo answers, drawn with the same bubble as the course chat. This module holds the Markdown
 * and KaTeX renderer, so the landing page loads it only when it is needed (see DemoCard).
 */
export default function DemoThread({ messages }: { messages: ChatMessage[] }) {
  // The demo has no model catalog. The model label travels in each message.
  const models = useMemo(() => ({ models: [], find: () => undefined, defaultModel: undefined }), []);
  return (
    <ModelsContext.Provider value={models}>
      <div className="space-y-7">
        {messages.map((message) => (
          <MessageBubble key={message.id} message={message} captions={DEMO_CAPTIONS} />
        ))}
      </div>
    </ModelsContext.Provider>
  );
}
