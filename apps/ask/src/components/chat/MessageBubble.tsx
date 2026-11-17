import { useDeferredValue, useMemo, useState } from 'react';
import { AlertCircle, CircleStop, Globe } from 'lucide-react';
import { Mascot, type MascotMood } from '@/components/ui/Mascot';
import { useModels } from '@/services/models/ModelsContext';
import { citedNumbers } from './citations';
import { MessageContent } from './MessageContent';
import { SourcesList } from './SourceBadge';
import { SourceHighlightContext } from './SourceHighlight';
import type { ChatMessage } from './useChatController';

export function MessageBubble({ message }: { message: ChatMessage }) {
  const { find } = useModels();
  const isUser = message.role === 'user';
  const modelInfo = message.model ? find(message.model) : undefined;
  const modelLabel = modelInfo?.label ?? message.model;
  const [activeSource, setActiveSource] = useState<number | null>(null);
  const streaming = message.state === 'streaming';
  const waiting = streaming && message.content === '';
  // While the answer streams, the list can lag one frame behind the text. It never blocks the text.
  const deferredContent = useDeferredValue(message.content);
  // Only the sources that the answer cites are the list. The renderer links the same markers.
  const { cited, other } = useMemo(() => {
    const all = message.sources ?? [];
    const numbers = citedNumbers(deferredContent, all);
    return {
      cited: numbers.flatMap((n) => all.filter((s) => s.n === n).slice(0, 1)),
      other: all.filter((s) => !numbers.includes(s.n)),
    };
  }, [deferredContent, message.sources]);

  const hasSources = !isUser && message.state !== 'failed' && message.content.trim() !== '' && (cited.length > 0 || other.length > 0);
  const mood: MascotMood = waiting || streaming ? 'thinking' : message.state === 'failed' ? 'puzzled' : (message.sources ?? []).length === 0 ? 'puzzled' : 'happy';

  if (isUser) {
    return (
      <div className="pt-4">
        <div data-testid="user-message" className="min-w-0">
          <span className="sr-only">You said:</span>
          <p className="whitespace-pre-wrap break-words border-l-2 border-accent/50 pl-4 font-serif text-[1.375rem] italic leading-snug text-text-primary">{message.content}</p>
        </div>
      </div>
    );
  }

  return (
    <SourceHighlightContext.Provider value={{ active: activeSource, onActive: setActiveSource }}>
      <div data-testid="assistant-message" className="grid min-w-0 gap-x-10 gap-y-4 @min-[56rem]:grid-cols-[minmax(0,var(--spacing-measure))_15rem]">
        <span className="sr-only">Ask said:</span>
        <div className="min-w-0">
          {waiting ? (
            <div className="flex h-7 items-center gap-1.5">
              <span className="sr-only">Ask is thinking</span>
              {[0, 1, 2].map((i) => (
                <span key={i} aria-hidden="true" className="h-2 w-2 rounded-full bg-accent/60 motion-safe:animate-bounce" style={{ animationDelay: `${i * 0.12}s` }} />
              ))}
            </div>
          ) : (
            <MessageContent content={message.content} sources={message.sources} className="text-text-primary" />
          )}
          {streaming && !waiting && <span className="stream-caret" aria-hidden="true" />}

          {message.state === 'stopped' && (
            <p className="mt-3 flex items-center gap-1.5 text-xs text-text-muted">
              <CircleStop className="h-3.5 w-3.5" aria-hidden="true" />
              Stopped. This answer may be incomplete.
            </p>
          )}
          {message.state === 'failed' && (
            <p role="alert" className="mt-3 flex items-start gap-1.5 text-sm text-danger">
              <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
              <span>
                {message.content ? 'The answer stopped early. ' : 'The answer failed. '}
                {message.error}
              </span>
            </p>
          )}
          <div className="mt-3 flex items-center gap-1.5 font-sans text-[11px] text-text-muted">
            <Mascot mood={mood} size={26} className="@min-[56rem]:hidden" />
            {modelLabel && (
              <p className="flex items-center gap-1.5" data-testid="message-model">
                {modelInfo?.residency === 'global' && <Globe className="h-3 w-3" aria-label="May process data outside Australia" />}
                {modelLabel}
              </p>
            )}
          </div>
        </div>
        <div className="min-w-0 space-y-3">
          <div className="hidden items-center gap-2 @min-[56rem]:flex">
            <Mascot mood={mood} size={40} />
            <p className="font-serif text-[0.8125rem] italic text-text-muted" aria-hidden="true">
              {MOOD_CAPTION[mood]}
            </p>
          </div>
          {hasSources && <SourcesList cited={cited} other={other} streaming={streaming} />}
        </div>
      </div>
    </SourceHighlightContext.Provider>
  );
}

const MOOD_CAPTION: Record<MascotMood, string> = {
  idle: '',
  thinking: 'Reading your pages...',
  happy: 'Found it in your pages.',
  puzzled: 'No pages used for this one.',
};
