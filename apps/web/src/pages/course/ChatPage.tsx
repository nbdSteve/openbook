import { useCallback, useEffect, useRef, useState } from 'react';
import { Link } from 'react-router';
import { X } from 'lucide-react';
import { Mascot } from '@/components/ui/Mascot';
import { Alert } from '@/components/ui/Alert';
import { Spinner } from '@/components/ui/Spinner';
import { Composer } from '@/components/chat/Composer';
import { MessageBubble } from '@/components/chat/MessageBubble';
import { styleName } from '@/lib/constants';
import { useResource } from '@/lib/useResource';
import { listDocuments } from '@/services/api/routes';
import { useMe } from '@/services/me/MeContext';
import { useChatCourse } from './CourseContext';

const NEAR_BOTTOM_PX = 120;

function documentsCount(n: number): string {
  return `${n} ${n === 1 ? 'document' : 'documents'}`;
}

function CourseMaterials({ courseId, role }: { courseId: string; role: 'owner' | 'student' }) {
  const docs = useResource(`documents:${courseId}`, (signal) => listDocuments(courseId, signal));
  if (!docs.data) return null;
  const ready = docs.data.filter((d) => d.status === 'ready').length;
  const preparing = docs.data.filter((d) => d.status === 'queued' || d.status === 'processing' || d.status === 'uploading').length;
  const cls = 'mx-auto mt-3 max-w-md text-sm text-text-tertiary';
  if (role === 'student') {
    return (
      <p className={cls} data-testid="materials">
        {ready > 0 ? `This course has ${documentsCount(ready)} that the tutor can use.` : 'The course owner has not added any documents yet. Answers will not use course materials until they do.'}
      </p>
    );
  }
  if (ready === 0 && preparing === 0) {
    return (
      <p className={cls} data-testid="materials">
        This course has no documents yet.{' '}
        <Link to={`/portal/courses/${courseId}`} className="font-medium text-accent underline">
          Add PDFs
        </Link>
      </p>
    );
  }
  return (
    <p className={cls} data-testid="materials">
      {ready > 0 ? `The tutor can use ${documentsCount(ready)}.` : 'Your documents are still being prepared.'}
      {ready > 0 && preparing > 0 ? ` ${preparing} more ${preparing === 1 ? 'is' : 'are'} still being prepared.` : ''}
    </p>
  );
}

export function ChatPage() {
  const { course, chat } = useChatCourse();
  const { me } = useMe();
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const streaming = chat.status === 'streaming';

  const [announcement, setAnnouncement] = useState('');
  const [previousStatus, setPreviousStatus] = useState(chat.status);
  if (previousStatus !== chat.status) {
    setPreviousStatus(chat.status);
    if (chat.status === 'streaming') setAnnouncement('The tutor is answering.');
    else if (previousStatus === 'streaming') {
      setAnnouncement(chat.messages[chat.messages.length - 1]?.state === 'failed' ? 'The answer failed.' : 'The answer is ready.');
    }
  }

  const onScroll = useCallback(() => {
    const el = scroller.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
  }, []);

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
      <div ref={scroller} onScroll={onScroll} className="@container relative min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex min-h-full w-full max-w-[60rem] flex-col px-4 py-6 sm:px-8 sm:py-8">
          {chat.status === 'loading' && <Spinner label="Loading conversation" />}
          {chat.loadError && (
            <Alert tone="error" className="mt-4">
              {chat.loadError}{' '}
              <Link to={`/ask/courses/${course.id}`} className="font-medium underline">
                Start a new chat
              </Link>
            </Alert>
          )}
          {empty && !chat.loadError && (
            <div className="my-auto py-10 text-center">
              <Mascot mood="idle" size={96} className="mx-auto mb-4" />
              <h2 className="font-serif text-title font-medium text-text-primary">What would you like to learn?</h2>
              <p className="mx-auto mt-3 max-w-md font-serif text-lg text-text-secondary">
                Ask anything about {course.name}. {chat.styleLocked ? `The tutor uses the ${styleName(chat.style)} style.` : `The tutor starts in the ${styleName(chat.style)} style. You can change it below.`}
              </p>
              <CourseMaterials courseId={course.id} role={course.role} />
            </div>
          )}
          <div className="space-y-8">
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
            style={chat.style}
            onStyleChange={chat.setStyle}
            styleLocked={chat.styleLocked}
            styleLockedReason={course.role === 'owner' ? 'Fixed in course settings' : 'Fixed by your teacher'}
            onSend={send}
            onStop={chat.stop}
            note={course.role === 'student' ? 'Your teacher can see the questions you ask in this course.' : undefined}
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
