import { Link, useParams } from 'react-router';
import { ArrowUp, ChevronDown, MessageSquare, Plus, RotateCcw, Settings2 } from 'lucide-react';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { Logo } from '@/components/ui/Logo';
import { Mascot } from '@/components/ui/Mascot';
import { ThemeToggle } from '@/components/ui/ThemeToggle';
import { MarginAnswer } from './MarginAnswer';
import { QUESTION } from './fixtures';

// A dev-only preview of the redesign with static data. The route exists only when `import.meta.env.DEV` is true.

const CHIPS = ['What is the quadratic formula?', 'Explain the zero product property', 'Why does a negative exponent flip the fraction?'];

function Masthead() {
  return (
    <header className="border-b border-border">
      <div className="mx-auto flex h-16 max-w-6xl items-center justify-between px-5 sm:px-8">
        <Logo />
        <nav aria-label="Main" className="flex items-center gap-1 sm:gap-2">
          <a href="#how" className="hidden rounded-lg px-3 py-2 text-sm text-text-secondary hover:text-text-primary md:block">How it works</a>
          <a href="#benchmarks" className="hidden rounded-lg px-3 py-2 text-sm text-text-secondary hover:text-text-primary md:block">Benchmarks</a>
          <a href="https://github.com/nbdSteve/openbook" className="hidden rounded-lg px-3 py-2 text-sm text-text-secondary hover:text-text-primary md:block">GitHub</a>
          <ThemeToggle />
          <Link to="/login" className={buttonStyles('ghost', 'sm', 'hidden sm:inline-flex')}>Sign in</Link>
          <Link to="/signup" className={buttonStyles('primary', 'sm')}>Get started</Link>
        </nav>
      </div>
    </header>
  );
}

function Question() {
  return (
    <>
      <p className="font-sans text-[0.6875rem] font-semibold uppercase tracking-[0.1em] text-text-muted">You asked</p>
      <p className="mt-1 font-serif text-2xl italic leading-snug text-text-primary">{QUESTION}</p>
    </>
  );
}

function Demo() {
  return (
    <section aria-labelledby="demo-title" className="rounded-2xl border border-border bg-surface shadow-md">
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 border-b border-border-subtle px-5 py-3 sm:px-7">
        <h2 id="demo-title" className="font-serif text-lg font-semibold text-text-primary">Try it now, no account needed</h2>
        <p className="text-xs text-text-muted">Answers from two OpenStax books. Model: Claude Sonnet 4.6</p>
      </div>
      <div className="px-5 pb-6 pt-5 sm:px-7">
        <MarginAnswer mood="happy" caption="Found it on two pages." header={<Question />} />
        <form className="mt-7 flex items-end gap-3 border-t border-border-subtle pt-5" onSubmit={(e) => e.preventDefault()}>
          <label className="min-w-0 flex-1">
            <span className="sr-only">Ask a follow-up question</span>
            <input
              placeholder="Ask a follow-up question"
              className="min-h-11 w-full border-0 border-b-2 border-border-strong bg-transparent px-1 font-serif text-lg text-text-primary placeholder:text-text-muted focus:border-accent focus:outline-none"
            />
          </label>
          <button type="submit" aria-label="Send" className={buttonStyles('primary', 'md', 'h-11 w-11 !px-0')}>
            <ArrowUp className="h-4 w-4" aria-hidden="true" />
          </button>
        </form>
        <div className="mt-4 flex flex-wrap items-center gap-2">
          {CHIPS.map((chip) => (
            <button key={chip} type="button" className="rounded-full border border-border bg-bg-primary px-3 py-1.5 text-left text-sm text-text-secondary hover:border-border-strong hover:text-text-primary">
              {chip}
            </button>
          ))}
          <button type="button" className="ml-auto inline-flex items-center gap-1.5 rounded-lg px-2 py-1.5 text-sm text-text-muted hover:text-text-primary">
            <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" />
            Start over
          </button>
        </div>
        <p className="mt-5 text-xs leading-relaxed text-text-muted">
          Sources: <em>Intermediate Algebra 2e</em> and <em>College Algebra 2e</em>, OpenStax, CC BY 4.0. Pages selected; text unchanged.
        </p>
      </div>
    </section>
  );
}

const BENCH = [
  { name: 'Hybrid (what Ask uses)', value: 0.94, strong: true },
  { name: 'Dense vectors only', value: 0.88, strong: false },
  { name: 'BM25 keywords only', value: 0.81, strong: false },
];

function Benchmarks() {
  return (
    <section id="benchmarks" aria-labelledby="bench-title" className="border-t border-border bg-bg-secondary">
      <div className="mx-auto grid max-w-6xl gap-10 px-5 py-16 sm:px-8 lg:grid-cols-[minmax(0,22rem)_1fr] lg:gap-16">
        <div>
          <p className="font-sans text-[0.6875rem] font-semibold uppercase tracking-[0.1em] text-accent">Benchmarks</p>
          <h2 id="bench-title" className="mt-2 font-serif text-title font-medium text-text-primary">Measured, <em>not claimed.</em></h2>
          <p className="mt-4 text-text-secondary">
            Measured on 107 questions over two OpenStax excerpts. The numbers below come from the commit that is running on this site.
          </p>
        </div>
        <figure>
          <figcaption className="mb-4 font-serif text-base italic text-text-secondary">
            Table 1. How often the right page is among the 8 passages the tutor reads (recall at 8).
          </figcaption>
          <ul className="divide-y divide-border border-y border-border">
            {BENCH.map((row) => (
              <li key={row.name} className="grid grid-cols-[minmax(0,1fr)_3.5rem] items-center gap-x-4 gap-y-2 py-3.5 sm:grid-cols-[13rem_1fr_3.5rem]">
                <span className={`text-sm ${row.strong ? 'font-semibold text-text-primary' : 'text-text-secondary'}`}>{row.name}</span>
                <span className="order-last col-span-2 h-2 rounded-full bg-bg-tertiary sm:order-none sm:col-span-1" aria-hidden="true">
                  <span className={`block h-full rounded-full ${row.strong ? 'bg-accent' : 'bg-text-muted/70'}`} style={{ width: `${row.value * 100}%` }} />
                </span>
                <span className="text-right font-serif text-xl tabular-nums text-text-primary">{row.value.toFixed(2)}</span>
              </li>
            ))}
          </ul>
        </figure>
      </div>
    </section>
  );
}

function Landing() {
  return (
    <div className="min-h-dvh bg-bg-primary">
      <Masthead />
      <main>
        <section className="mx-auto max-w-6xl px-5 pb-16 pt-12 sm:px-8 sm:pt-16">
          <p className="font-sans text-[0.6875rem] font-semibold uppercase tracking-[0.1em] text-accent">Open-source tutoring from your own books</p>
          <h1 className="mt-3 max-w-4xl font-serif text-display font-medium tracking-[-0.02em] text-text-primary">
            A tutor that <em className="text-accent">shows its sources.</em>
          </h1>
          <p className="mt-5 max-w-3xl font-serif text-xl leading-relaxed text-text-secondary">
            Ask answers from the pages you give it, and points to the page it used. Try it on two real textbooks below.
          </p>
          <div className="mt-10">
            <Demo />
          </div>
        </section>
        <Benchmarks />
      </main>
    </div>
  );
}

const CONVERSATIONS = ['How do I solve x² − 5x + 6 = 0?', 'What is the discriminant?', 'Completing the square, step by step'];

function Sidebar() {
  return (
    <aside aria-label="Course navigation" className="hidden w-72 shrink-0 flex-col border-r border-border bg-bg-secondary lg:flex">
      <div className="flex h-14 items-center border-b border-border px-4">
        <Logo to="/app" />
      </div>
      <div className="space-y-1 p-3">
        <a className="flex items-center gap-2 rounded-xl px-3 py-2 text-sm text-text-secondary hover:bg-surface-hover" href="#">All courses</a>
        <a className={buttonStyles('primary', 'md', 'w-full')} href="#"><Plus className="h-4 w-4" aria-hidden="true" />New chat</a>
        <a className="flex items-center gap-2 rounded-xl px-3 py-2.5 text-sm text-text-secondary hover:bg-surface-hover" href="#"><Settings2 className="h-4 w-4" aria-hidden="true" />Manage course</a>
      </div>
      <nav aria-label="Conversations" className="min-h-0 flex-1 px-3">
        <h2 className="px-3 pb-2 pt-1 font-sans text-[0.6875rem] font-semibold uppercase tracking-[0.1em] text-text-muted">Earlier questions</h2>
        <ul className="space-y-0.5">
          {CONVERSATIONS.map((title, i) => (
            <li key={title}>
              <a href="#" aria-current={i === 0 ? 'page' : undefined} className={`flex items-start gap-2 rounded-xl py-2 pl-3 pr-3 text-sm ${i === 0 ? 'bg-surface font-medium text-text-primary shadow-sm' : 'text-text-secondary hover:bg-surface-hover'}`}>
                <MessageSquare className={`mt-0.5 h-4 w-4 shrink-0 ${i === 0 ? 'text-accent' : 'text-text-muted'}`} aria-hidden="true" />
                <span className="line-clamp-2">{title}</span>
              </a>
            </li>
          ))}
        </ul>
      </nav>
      <p className="border-t border-border p-4 text-xs text-text-muted">teacher@openbook.test</p>
    </aside>
  );
}

function Chat() {
  return (
    <div className="flex h-dvh overflow-hidden bg-bg-primary">
      <Sidebar />
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border bg-bg-secondary px-4">
          <div className="lg:hidden [&_span]:sr-only"><Logo to="/app" /></div>
          <h1 className="min-w-0 flex-1 truncate font-serif text-lg font-semibold text-text-primary lg:text-xl">Algebra 2</h1>
          <span className="rounded-full bg-accent-subtle px-2.5 py-0.5 text-xs font-medium text-accent">Step by step</span>
          <ThemeToggle />
        </header>
        <main className="min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto max-w-[62rem] px-5 py-8 sm:px-8 sm:py-10">
            <MarginAnswer mood="happy" caption="Found it on two pages." header={<Question />} />
            <p className="mt-6 flex items-center gap-1.5 text-[0.6875rem] text-text-muted" data-testid="message-model">Claude Sonnet 5.5</p>
          </div>
        </main>
        <div className="shrink-0 border-t border-border bg-bg-primary px-4 pb-3 pt-3">
          <div className="mx-auto max-w-[62rem]">
            <div className="flex items-end gap-2 rounded-2xl border border-border-strong bg-surface p-2 shadow-sm">
              <textarea rows={1} aria-label="Message to Ask" placeholder="Ask a question" className="min-h-11 flex-1 resize-none bg-transparent px-3 py-2.5 font-serif text-lg text-text-primary placeholder:text-text-muted focus:outline-none" />
              <button type="button" aria-label="Send message" className="flex h-11 w-11 items-center justify-center rounded-xl bg-bg-tertiary text-text-muted"><ArrowUp className="h-4 w-4" aria-hidden="true" /></button>
            </div>
            <div className="mt-1.5 flex items-center justify-between text-xs text-text-muted">
              <span className="inline-flex items-center gap-1 rounded-lg border border-border-strong bg-surface px-3 py-1.5 font-medium text-text-secondary">Claude Sonnet 5.5<ChevronDown className="h-3.5 w-3.5" aria-hidden="true" /></span>
              <span>12 / 50 messages today</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function Moods() {
  return (
    <div className="flex min-h-dvh items-center justify-center bg-bg-primary p-8">
      <ul className="flex flex-wrap gap-8">
        {(['idle', 'thinking', 'happy', 'puzzled'] as const).map((mood) => (
          <li key={mood} className="flex flex-col items-center gap-3">
            <Mascot mood={mood} size={132} />
            <span className="font-serif text-lg italic text-text-secondary">{mood}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

export function DesignPage() {
  const { screen = 'hero' } = useParams();
  if (screen === 'chat') return <Chat />;
  if (screen === 'moods') return <Moods />;
  return <Landing />;
}
