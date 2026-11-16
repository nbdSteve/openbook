import { useCallback, useEffect, useRef } from 'react';
import { Link } from 'react-router';
import { Sparkles, X } from 'lucide-react';
import { Alert } from '@/components/ui/Alert';
import { Spinner } from '@/components/ui/Spinner';
import { Composer } from '@/components/chat/Composer';
import { MessageBubble } from '@/components/chat/MessageBubble';
import { styleName } from '@/lib/constants';
import { useMe } from '@/services/me/MeContext';
import { useCourse } from './CourseContext';

const NEAR_BOTTOM_PX = 120;

export function ChatPage() {
  const { course, chat } = useCourse();
  const { me } = useMe();
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const streaming = chat.status === 'streaming';

  const onScroll = useCallback(() => {
    const el = scroller.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
  }, []);

  // Follow the answer while the reader is at the bottom. Do not fight a reader who scrolled up.
  const last = chat.messages[chat.messages.length - 1];
  useEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [chat.messages.length, last?.content]);

  const send = useCallback(
    (text: string) => {
      stick.current = true;
      return chat.send(text);
    },
    [chat],
  );

  const empty = chat.messages.length === 0 && chat.status !== 'loading';

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div ref={scroller} onScroll={onScroll} className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex min-h-full w-full max-w-3xl flex-col px-3 py-4 sm:px-4">
          {chat.status === 'loading' && <Spinner label="Loading conversation" />}
          {chat.loadError && (
            <Alert tone="error" className="mt-4">
              {chat.loadError}{' '}
              <Link to={`/app/c/${course.id}`} className="font-medium underline">
                Start a new chat
              </Link>
            </Alert>
          )}
          {empty && !chat.loadError && (
            <div className="my-auto py-10 text-center">
              <div className="mx-auto mb-5 flex h-14 w-14 items-center justify-center rounded-2xl bg-accent-subtle">
                <Sparkles className="h-7 w-7 text-accent" aria-hidden="true" />
              </div>
              <h2 className="text-2xl font-bold text-text-primary sm:text-3xl">What would you like to learn?</h2>
              <p className="mx-auto mt-3 max-w-md text-text-secondary">
                Ask a question about {course.name}. Ask answers in the {styleName(course.tutoringStyle)} style.
              </p>
              {course.role === 'owner' && (
                <p className="mx-auto mt-3 max-w-md text-sm text-text-tertiary">
                  Answers work best when the course has materials.{' '}
                  <Link to={`/app/c/${course.id}/manage`} className="font-medium text-accent underline">
                    Add PDFs
                  </Link>
                </p>
              )}
            </div>
          )}
          <div className="space-y-4">
            {chat.messages.map((message) => (
              <MessageBubble key={message.id} message={message} />
            ))}
          </div>
        </div>
      </div>

      <div className="shrink-0 border-t border-border bg-bg-primary px-3 pb-3 pt-3 sm:px-4">
        <div className="mx-auto w-full max-w-3xl">
          {chat.sendError && (
            <div className="mb-3 flex items-start gap-2">
              <Alert tone="error" className="flex-1">
                {chat.sendError}
              </Alert>
              <button
                type="button"
                onClick={chat.dismissSendError}
                className="rounded-lg p-2 text-text-tertiary hover:bg-surface-hover"
                aria-label="Dismiss message"
              >
                <X className="h-4 w-4" aria-hidden="true" />
              </button>
            </div>
          )}
          <Composer
            streaming={streaming}
            usage={me.usage}
            model={chat.model}
            onModelChange={chat.setModel}
            onSend={send}
            onStop={chat.stop}
          />
        </div>
      </div>
    </div>
  );
}
