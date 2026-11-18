import { useMemo } from 'react';
import { DEMO_CAPTIONS } from '@/components/chat/captions';
import { MessageBubble } from '@/components/chat/MessageBubble';
import type { ChatMessage } from '@/components/chat/useChatController';
import { ModelsContext } from '@/services/models/ModelsContext';

export default function DemoThread({ messages }: { messages: ChatMessage[] }) {
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
