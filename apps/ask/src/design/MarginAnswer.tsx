import type { ReactNode } from 'react';
import { Mascot, type MascotMood } from '@/components/ui/Mascot';
import { NOTES, ROWS, type Note } from './fixtures';

function MarginNote({ note }: { note: Note }) {
  return (
    <aside className="relative border-l-2 border-accent/40 pl-3 font-serif text-note text-text-secondary lg:ml-0">
      <p className="mb-0.5 font-sans text-[0.6875rem] font-semibold uppercase tracking-[0.08em] text-text-muted">
        <span className="mr-1.5 inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-accent-subtle px-1 text-accent">{note.n}</span>
        {note.book}, p. {note.page}
      </p>
      <p className="italic">{note.snippet}</p>
    </aside>
  );
}

/**
 * An answer set as a page: the text in a reading column, and the sources it cites as notes in the
 * right margin, level with the paragraph that cites them. On small screens the notes sit under
 * the paragraph, like footnotes.
 */
export function MarginAnswer({ mood = 'happy', caption, header }: { mood?: MascotMood; caption: string; header: ReactNode }) {
  return (
    <div className="grid gap-y-5 lg:grid-cols-[minmax(0,var(--spacing-measure))_15.5rem] lg:gap-x-10">
      <div className="min-w-0 pb-1">{header}</div>
      <div className="hidden items-start gap-2 lg:flex">
        <Mascot mood={mood} size={40} className="mt-1" />
        <p className="pt-2.5 font-serif text-note italic text-text-muted">{caption}</p>
      </div>
      {ROWS.map((row, i) => (
        <div key={i} className="contents">
          <div className="font-serif text-reading text-text-primary [&_p]:my-0 [&_p+p]:mt-3 [&_.katex]:text-[1.04em]">
            {row.body}
          </div>
          <div className="space-y-3 pl-1 lg:pl-0">
            {row.notes.map((n) => (
              <MarginNote key={n} note={NOTES[n - 1]} />
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
