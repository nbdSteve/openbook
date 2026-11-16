import { useCallback, useEffect, useRef, useState } from 'react';
import { Link } from 'react-router';
import { Sparkles, X } from 'lucide-react';
import { Alert } from '@/components/ui/Alert';
import { Spinner } from '@/components/ui/Spinner';
import { Composer } from '@/components/chat/Composer';
import { MessageBubble } from '@/components/chat/MessageBubble';
import { styleName } from '@/lib/constants';
import { useResource } from '@/lib/useResource';
import { listDocuments } from '@/services/api/routes';
import { useMe } from '@/services/me/MeContext';
import { useCourse } from './CourseContext';

const NEAR_BOTTOM_PX = 120;

/** Owners with no documents yet get a nudge. Owners with documents see nothing. */
function AddMaterialsHint({ courseId }: { courseId: string }) {
  const docs = useResource(`documents:${courseId}`, (signal) => listDocuments(courseId, signal));
  if (!docs.data || docs.data.length > 0) return null;
  return (
    <p className="mx-auto mt-3 max-w-md text-sm text-text-tertiary">
      This course has no materials yet.{' '}
      <Link to={`/app/c/${courseId}/manage`} className="font-medium text-accent underline">
        Add PDFs
      </Link>
    </p>
  );
}

export function ChatPage() {
  const { course, chat } = useCourse();
  const { me } = useMe();
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const streaming = chat.status === 'streaming';

  // One polite live region announces the start and the end of an answer. Reading every token would be noise.
  const [announcement, setAnnouncement] = useState('');
  const [previousStatus, setPreviousStatus] = useState(chat.status);
  if (previousStatus !== chat.status) {
    setPreviousStatus(chat.status);
    if (chat.status === 'streaming') setAnnouncement('Ask is answering.');
    else if (previousStatus === 'streaming') {
      setAnnouncement(chat.messages[chat.messages.length - 1]?.state === 'failed' ? 'The answer failed.' : 'The answer is ready.');
    }
  }

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
      <div role="status" aria-live="polite" className="sr-only">
        {announcement}
      </div>
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
                Ask anything about {course.name}. The tutor uses the {styleName(course.tutoringStyle)} style.
              </p>
              {course.role === 'owner' && <AddMaterialsHint courseId={course.id} />}
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
            disabledReason={
              chat.status === 'loading'
                ? 'Loading the conversation'
                : chat.loadError
                  ? 'This conversation could not be loaded'
                  : undefined
            }
          />
        </div>
      </div>
    </div>
  );
}
