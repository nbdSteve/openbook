import { useDeferredValue, useMemo, useState } from 'react';
import { clsx } from 'clsx';
import { AlertCircle, CircleStop, Globe } from 'lucide-react';
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

  return (
    <SourceHighlightContext.Provider value={{ active: activeSource, onActive: setActiveSource }}>
    <div className={clsx('flex w-full', isUser ? 'justify-end' : 'justify-start')}>
      <div className={clsx('flex min-w-0 max-w-[94%] items-start gap-2.5 sm:max-w-[85%] sm:gap-3', isUser && 'flex-row-reverse')}>
        <div className="mt-1 shrink-0">
          {isUser ? (
            <div
              aria-hidden="true"
              className="flex h-8 w-8 items-center justify-center rounded-full bg-accent text-xs font-semibold text-on-accent"
            >
              You
            </div>
          ) : (
            <img src="/ask-logo.webp" alt="" className="h-8 w-8 rounded-full object-cover shadow-sm" />
          )}
        </div>
        <div className="min-w-0" data-testid={isUser ? 'user-message' : 'assistant-message'}>
          <span className="sr-only">{isUser ? 'You said:' : 'Ask said:'}</span>
          <div
            className={clsx(
              'rounded-2xl px-4 py-3 shadow-sm',
              isUser
                ? 'rounded-tr-md bg-accent text-on-accent'
                : 'rounded-tl-md border border-border bg-surface text-text-primary',
            )}
          >
            {isUser ? (
              <p className="whitespace-pre-wrap break-words text-[0.95rem] leading-relaxed">{message.content}</p>
            ) : (
              <div>
                {waiting ? (
                  <div className="flex h-6 items-center gap-1.5">
                    <span className="sr-only">Ask is thinking</span>
                    {[0, 1, 2].map((i) => (
                      <span
                        key={i}
                        aria-hidden="true"
                        className="h-2 w-2 animate-bounce rounded-full bg-accent/60"
                        style={{ animationDelay: `${i * 0.12}s` }}
                      />
                    ))}
                  </div>
                ) : (
                  <MessageContent content={message.content} sources={message.sources} className="text-text-primary" />
                )}
                {streaming && !waiting && <span className="stream-caret" aria-hidden="true" />}
              </div>
            )}

            {!isUser && message.state !== 'failed' && message.content.trim() !== '' && <SourcesList cited={cited} other={other} streaming={streaming} />}

            {message.state === 'stopped' && (
              <p className="mt-2 flex items-center gap-1.5 text-xs text-text-muted">
                <CircleStop className="h-3.5 w-3.5" aria-hidden="true" />
                Stopped. This answer may be incomplete.
              </p>
            )}
            {message.state === 'failed' && (
              <p role="alert" className="mt-2 flex items-start gap-1.5 text-sm text-danger">
                <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                <span>
                  {message.content ? 'The answer stopped early. ' : 'The answer failed. '}
                  {message.error}
                </span>
              </p>
            )}
          </div>
          {!isUser && modelLabel && (
            <p className="mt-1 flex items-center gap-1 px-1 text-[11px] text-text-muted" data-testid="message-model">
              {modelInfo?.residency === 'global' && <Globe className="h-3 w-3" aria-label="May process data outside Australia" />}
              {modelLabel}
            </p>
          )}
        </div>
      </div>
    </div>
    </SourceHighlightContext.Provider>
  );
}
