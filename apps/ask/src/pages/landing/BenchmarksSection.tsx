import { ArrowUpRight } from 'lucide-react';
import { benchmarks, benchmarksSource, METHOD_URL, percent, type RetrievalMode } from '@/benchmarks';
import { cn } from '@/lib/cn';

const MODES: { key: RetrievalMode; label: string }[] = [
  { key: 'hybrid', label: 'Hybrid' },
  { key: 'dense', label: 'Dense' },
  { key: 'bm25', label: 'BM25' },
];

const WORDS = ['zero', 'one', 'two', 'three', 'four', 'five'];

/** A ring that fills to `value` (0 to 1). The number sits in the middle. */
function Ring({ value, label, tone }: { value: number; label: string; tone: 'accent' | 'sage' }) {
  const r = 42;
  const c = 2 * Math.PI * r;
  return (
    <div className="flex flex-col items-center gap-3 text-center">
      <div className="relative h-28 w-28 sm:h-32 sm:w-32">
        <svg viewBox="0 0 100 100" className="h-full w-full -rotate-90" aria-hidden="true">
          <circle cx="50" cy="50" r={r} fill="none" strokeWidth="9" className="stroke-bg-tertiary" />
          <circle
            cx="50"
            cy="50"
            r={r}
            fill="none"
            strokeWidth="9"
            strokeLinecap="round"
            strokeDasharray={`${Math.max(value * c, value > 0 ? 3 : 0)} ${c}`}
            className={tone === 'accent' ? 'stroke-accent' : 'stroke-teal'}
          />
        </svg>
        <span className="absolute inset-0 flex items-center justify-center font-serif text-[1.75rem] font-medium tabular-nums text-text-primary">{percent(value)}</span>
      </div>
      <span className="text-sm text-text-secondary">{label}</span>
    </div>
  );
}

/** The retrieval numbers of this build, shown as bars and rings. */
export function BenchmarksSection() {
  const { retrieval, gate, questions, corpora } = benchmarks;
  const corporaWord = WORDS[corpora.length] ?? String(corpora.length);
  return (
    <section id="benchmarks" aria-labelledby="bench-title" className="border-y border-border bg-bg-secondary">
      <div className="mx-auto max-w-6xl px-5 py-16 sm:px-8 sm:py-20">
        <h2 id="bench-title" className="sr-only">
          Benchmarks
        </h2>
        <div className="grid gap-5 lg:grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)]">
          <figure className="rounded-3xl border border-border bg-surface p-6 shadow-sm sm:p-8">
            <figcaption className="sr-only">Share of questions where the right page is among the 8 passages the tutor reads</figcaption>
            <p className="font-serif text-[clamp(4rem,10vw,6.5rem)] font-medium leading-none tracking-tight text-accent tabular-nums">{percent(retrieval.hybrid.recallAt8)}</p>
            <p className="mt-2 font-serif text-xl italic text-text-secondary">right page in the top 8</p>
            <ul className="mt-7 space-y-3.5">
              {MODES.map(({ key, label }) => (
                <li key={key} className="grid grid-cols-[4.5rem_minmax(0,1fr)_3rem] items-center gap-3 text-sm">
                  <span className={cn(key === 'hybrid' ? 'font-semibold text-text-primary' : 'text-text-secondary')}>{label}</span>
                  <span className="h-3 overflow-hidden rounded-full bg-bg-tertiary" aria-hidden="true">
                    <span className={cn('block h-full rounded-full', key === 'hybrid' ? 'bg-accent' : 'bg-text-muted/60')} style={{ width: `${retrieval[key].recallAt8 * 100}%` }} />
                  </span>
                  <span className="text-right font-serif text-lg tabular-nums text-text-primary">{percent(retrieval[key].recallAt8)}</span>
                </li>
              ))}
            </ul>
          </figure>
          <div className="flex flex-row items-center justify-around gap-4 rounded-3xl border border-border bg-surface p-6 shadow-sm sm:p-8 lg:flex-col lg:justify-center lg:gap-8">
            <Ring value={gate.offTopicCaught} label="Off-topic caught" tone="accent" />
            <Ring value={gate.falseGateRate} label="False gates" tone="sage" />
          </div>
        </div>
        <p className="mt-6 flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-text-secondary">
          <span>
            {questions} questions, {corporaWord} OpenStax excerpts.
          </span>
          <a href={METHOD_URL} target="_blank" rel="noopener noreferrer" className="inline-flex items-center gap-1 font-medium text-accent underline underline-offset-2 hover:text-accent-hover">
            How we measure
            <ArrowUpRight className="h-3.5 w-3.5" aria-hidden="true" />
          </a>
          {benchmarksSource === 'fixture' && <span className="rounded-full bg-warning-light px-2.5 py-0.5 text-xs font-medium text-warning">Sample numbers</span>}
        </p>
      </div>
    </section>
  );
}
