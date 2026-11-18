import { lazy, Suspense, useRef, useState, type FormEvent } from 'react';
import { Link } from 'react-router';
import { ArrowUp, RotateCcw, Square } from 'lucide-react';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { StylePicker } from '@/components/chat/StylePicker';
import { Mascot } from '@/components/ui/Mascot';
import { cn } from '@/lib/cn';
import type { DemoConfig } from '@/config/loadConfig';
import { DEMO_MAX_QUESTION } from '@/services/demo/demoClient';
import { ExampleAnswer } from './ExampleAnswer';
import { useDemoChat } from './useDemoChat';

const loadThread = () => import('./DemoThread');
const DemoThread = lazy(loadThread);

const FALLBACK_EXAMPLES = ['Factor x² − 5x + 6', 'Who can veto a bill?', 'What is a function?'];

function ThreadSkeleton() {
  return (
    <div className="min-h-[12rem] space-y-4 py-2" role="status">
      <span className="sr-only">Loading the answer</span>
      <div className="h-7 w-2/3 animate-pulse rounded-lg bg-bg-tertiary" />
      <div className="h-5 w-full max-w-[38rem] animate-pulse rounded-lg bg-bg-tertiary" />
      <div className="h-5 w-full max-w-[34rem] animate-pulse rounded-lg bg-bg-tertiary" />
    </div>
  );
}

interface Credit {
  title: string;
  publisher?: string;
  license: string;
  url?: string;
}

function Credits({ credits }: { credits: Credit[] }) {
  if (credits.length === 0) return null;
  return (
    <p className="mt-5 text-xs leading-relaxed text-text-muted">
      {credits.map((credit, i) => (
        <span key={credit.title}>
          {i > 0 && '; '}
          {credit.url ? (
            <a href={credit.url} target="_blank" rel="noopener noreferrer" className="underline underline-offset-2 hover:text-text-primary">
              {credit.title}
            </a>
          ) : (
            credit.title
          )}
          {credit.publisher && !credit.title.includes(credit.publisher) ? `, ${credit.publisher}` : ''}
          {credit.license ? `, ${credit.license}` : ''}
        </span>
      ))}
    </p>
  );
}

export function DemoCard({ demo }: { demo: DemoConfig | undefined }) {
  const live = demo?.enabled === true;
  const chat = useDemoChat();
  const [value, setValue] = useState('');
  const input = useRef<HTMLInputElement>(null);
  const started = chat.messages.length > 0;
  const showExample = !started;
  const streaming = chat.status === 'streaming';
  const examples = demo?.examples && demo.examples.length > 0 ? demo.examples.slice(0, 3) : FALLBACK_EXAMPLES;

  const [announcement, setAnnouncement] = useState('');
  const [previous, setPrevious] = useState(chat.status);
  if (previous !== chat.status) {
    setPrevious(chat.status);
    if (chat.status === 'streaming') setAnnouncement('The tutor is answering.');
    else setAnnouncement(chat.messages[chat.messages.length - 1]?.state === 'failed' ? 'The answer failed.' : 'The answer is ready.');
  }

  const submit = async (text: string) => {
    void loadThread();
    const accepted = await chat.ask(text);
    if (accepted) setValue('');
  };
  const onSubmit = (event: FormEvent) => {
    event.preventDefault();
    if (value.trim() && !streaming) {
      void loadThread();
      const text = value;
      setValue('');
      void chat.ask(text).then((accepted) => {
        if (!accepted) setValue((current) => (current === '' ? text : current));
      });
    }
  };

  const credits: Credit[] =
    chat.attribution.length > 0
      ? chat.attribution.map((a) => ({ title: a.title, publisher: a.publisher, license: a.license, url: a.sourceUrl }))
      : (demo?.sources ?? []).map((source) => ({ title: source.title, license: source.license, url: source.url }));

  return (
    <section aria-label="Live demo" className="@container rounded-3xl border border-border bg-surface shadow-lg">
      <div role="status" className="sr-only">
        {announcement}
      </div>
      <div className="flex items-center justify-between gap-3 border-b border-border-subtle px-5 py-3 sm:px-8">
        <p className="flex items-center gap-2 font-sans text-[0.6875rem] font-semibold uppercase tracking-[0.1em] text-text-muted">
          <span aria-hidden="true" className={cn('h-2 w-2 rounded-full', live ? 'bg-success motion-safe:animate-pulse' : 'bg-border-strong')} />
          <span className="whitespace-nowrap">
            {live ? 'Live demo' : 'Example'}
            {live && demo?.model && (
              <span className="font-medium normal-case tracking-normal">
                {' · '}
                <span className="sm:hidden">{demo.model.label.replace(/^Claude\s+/, '')}</span>
                <span className="hidden sm:inline">{demo.model.label}</span>
              </span>
            )}
          </span>
        </p>
        {showExample && live && <p className="font-sans text-[0.6875rem] font-semibold uppercase tracking-[0.1em] text-text-muted">Example</p>}
        {started && (
          <button
            type="button"
            onClick={() => {
              chat.reset();
              setValue('');
              input.current?.focus();
            }}
            className="inline-flex min-h-9 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-lg px-2 text-sm text-text-muted hover:text-text-primary"
          >
            <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" />
            Start over
          </button>
        )}
      </div>

      <div className="px-5 pb-2 pt-5 sm:px-8 sm:pt-7" data-testid="demo-thread">
        {showExample ? (
          <ExampleAnswer />
        ) : (
          <Suspense fallback={<ThreadSkeleton />}>
            <DemoThread messages={chat.messages} />
          </Suspense>
        )}
        {chat.notice && <p className="mt-4 font-serif text-sm italic text-text-muted">{chat.notice}</p>}
      </div>

      {live ? (
        <div className="px-5 pb-5 sm:px-8 sm:pb-6">
          <form onSubmit={onSubmit} className="mt-5 flex items-end gap-3 border-t border-border-subtle pt-5">
            <label className="min-w-0 flex-1">
              <span className="sr-only">Your question</span>
              <input
                ref={input}
                onFocus={() => void loadThread()}
                value={value}
                onChange={(e) => setValue(e.target.value)}
                maxLength={DEMO_MAX_QUESTION}
                autoComplete="off"
                placeholder={started ? 'Ask a follow-up' : 'Ask a question'}
                className="min-h-11 w-full rounded-none border-0 border-b-2 border-border-strong bg-transparent px-1 font-serif text-xl text-text-primary placeholder:text-text-muted focus:border-accent focus:outline-none"
              />
            </label>
            {streaming ? (
              <button type="button" onClick={chat.stop} aria-label="Stop answer" className={buttonStyles('danger', 'md', 'h-11 w-11 shrink-0 !px-0')}>
                <Square className="h-4 w-4 fill-current" aria-hidden="true" />
              </button>
            ) : (
              <button type="submit" disabled={!value.trim()} aria-label="Send question" className={buttonStyles('primary', 'md', 'h-11 w-11 shrink-0 !px-0')}>
                <ArrowUp className="h-4 w-4" aria-hidden="true" />
              </button>
            )}
          </form>

          {chat.problem && (
            <div
              role={chat.problem.kind === 'rate_limited' || chat.problem.kind === 'busy' || chat.problem.kind === 'unavailable' ? 'status' : 'alert'}
              data-testid="demo-problem"
              data-kind={chat.problem.kind}
              className="mt-4 flex items-center gap-3 rounded-xl bg-bg-secondary px-4 py-3"
            >
              <Mascot mood="puzzled" size={44} />
              <p className="min-w-0 flex-1 font-serif text-base text-text-primary">{chat.problem.message}</p>
              {(chat.problem.kind === 'busy' || chat.problem.kind === 'unavailable') && (
                <Link to="/signup" className={buttonStyles('secondary', 'sm', 'shrink-0')}>
                  Sign up
                </Link>
              )}
            </div>
          )}

          <div className="mt-4">
            <StylePicker value={chat.style} onChange={chat.setStyle} disabled={streaming} placement="down" />
          </div>

          <ul className="-mx-5 mt-4 flex gap-2 overflow-x-auto px-5 pb-1 [mask-image:linear-gradient(to_right,transparent,black_1.25rem,black_calc(100%-2rem),transparent)] sm:mx-0 sm:flex-wrap sm:overflow-visible sm:px-0 sm:[mask-image:none]" aria-label="Example questions">
            {examples.map((example) => (
              <li key={example} className="shrink-0">
                <button
                  type="button"
                  aria-disabled={streaming || undefined}
                  onClick={() => !streaming && void submit(example)}
                  className="min-h-9 whitespace-nowrap rounded-full border border-border bg-bg-primary px-3.5 py-1.5 text-left text-sm text-text-secondary transition-colors hover:border-border-strong hover:text-text-primary aria-disabled:opacity-50"
                >
                  {example}
                </button>
              </li>
            ))}
          </ul>

          <Credits credits={credits} />
        </div>
      ) : (
        <div className="flex px-5 pb-6 pt-3 sm:px-8">
          <Link to="/signup" className={buttonStyles('primary', 'md')}>
            Ask your own books
          </Link>
        </div>
      )}
    </section>
  );
}
