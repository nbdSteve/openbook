import type { ReactNode } from 'react';
import { Mascot, type MascotMood } from '@/components/ui/Mascot';
import { COURSE_CAPTIONS } from './captions';

interface AnswerFrameProps {
  /** The answer text, and any note about how it ended. */
  body: ReactNode;
  /** The line after the answer, for example the model name. The mascot sits before it on narrow screens. */
  footer?: ReactNode;
  /** The sources of the answer. On wide screens they are notes in the right margin. */
  sources?: ReactNode;
  mood: MascotMood;
  captions?: Record<MascotMood, string>;
}

/**
 * The page layout of one answer: the text in a reading column, and the sources as notes in the
 * right margin when the column is wide enough (a container query on the chat column). The mascot
 * sits at the head of the margin, or before the footer on narrow screens. The course chat and
 * the landing page example both use it, so they look the same.
 */
export function AnswerFrame({ body, footer, sources, mood, captions = COURSE_CAPTIONS }: AnswerFrameProps) {
  return (
    <div data-testid="assistant-message" className="grid min-w-0 gap-x-10 gap-y-4 @min-[56rem]:grid-cols-[minmax(0,var(--spacing-measure))_15rem]">
      <span className="sr-only">Ask said:</span>
      <div className="min-w-0">
        {body}
        <div className="mt-3 flex items-center gap-1.5 font-sans text-[11px] text-text-muted">
          <Mascot mood={mood} size={26} className="@min-[56rem]:hidden" />
          {footer}
        </div>
      </div>
      <div className="min-w-0 space-y-3">
        <div className="hidden items-center gap-2 @min-[56rem]:flex">
          <Mascot mood={mood} size={40} />
          <p className="font-serif text-[0.8125rem] italic text-text-muted" aria-hidden="true">
            {captions[mood]}
          </p>
        </div>
        {sources}
      </div>
    </div>
  );
}
