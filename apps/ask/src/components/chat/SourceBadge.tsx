import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { cn } from '@/lib/cn';
import { sourceLabel } from './citations';
import { useSourceHighlight } from './SourceHighlight';
import type { Source } from '@/services/api/types';

interface Position {
  left: number;
  width: number;
  top?: number;
  bottom?: number;
  maxHeight: number;
}

const POPOVER_WIDTH = 340;

interface SourceButtonProps {
  source: Source;
  /** `badge` is the inline [n] mark. `entry` is a row in the sources list. */
  variant: 'badge' | 'entry';
}

/**
 * A button that shows the source passage on hover, focus, or tap (a click pins it open; Escape
 * closes it). The passage is shown as plain text, never as HTML or Markdown. The popover is fixed
 * and clamped to the viewport, so it never causes horizontal overflow. A badge and its list entry
 * share `active`, so hovering or focusing one highlights the other.
 */
export function SourceButton({ source, variant }: SourceButtonProps) {
  const { active, onActive } = useSourceHighlight();
  const [position, setPosition] = useState<Position | null>(null);
  // A click or tap pins the popover open. Touch browsers send mouseenter before click.
  const pinned = useRef(false);
  const button = useRef<HTMLButtonElement>(null);
  const popoverId = useId();
  const open = position !== null;
  const highlighted = active === source.n;

  const show = () => {
    onActive(source.n);
    const rect = button.current?.getBoundingClientRect();
    if (!rect) return;
    const width = Math.min(POPOVER_WIDTH, window.innerWidth - 16);
    const left = Math.min(Math.max(8, rect.left + rect.width / 2 - width / 2), window.innerWidth - width - 8);
    // Open on the side with more room, and never taller than that room.
    const roomAbove = rect.top - 16;
    const roomBelow = window.innerHeight - rect.bottom - 16;
    setPosition(
      roomAbove > roomBelow
        ? { left, width, bottom: window.innerHeight - rect.top + 8, maxHeight: roomAbove }
        : { left, width, top: rect.bottom + 8, maxHeight: roomBelow },
    );
  };
  const hide = () => {
    pinned.current = false;
    onActive(null);
    setPosition(null);
  };

  useEffect(() => {
    if (!open) return;
    const close = () => {
      pinned.current = false;
      onActive(null);
      setPosition(null);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
    };
    // Follow the button when the page scrolls or the layout shifts. Close if it leaves the screen.
    const follow = () => {
      const rect = button.current?.getBoundingClientRect();
      if (!rect || rect.bottom < 0 || rect.top > window.innerHeight) close();
      else show();
    };
    window.addEventListener('scroll', follow, true);
    window.addEventListener('resize', follow);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('scroll', follow, true);
      window.removeEventListener('resize', follow);
      window.removeEventListener('keydown', onKey);
    };
    // `show` and `onActive` change on every render. The listeners only need to follow `open`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const content: ReactNode =
    variant === 'badge' ? (
      source.n
    ) : (
      <>
        <span className="flex h-5 min-w-5 shrink-0 items-center justify-center rounded-md bg-teal-subtle px-1 text-[11px] font-bold text-teal">{source.n}</span>
        <span className="min-w-0 truncate">{source.documentName}</span>
        {source.page !== undefined && <span className="shrink-0">, p. {source.page}</span>}
      </>
    );

  return (
    <>
      <button
        ref={button}
        type="button"
        data-source-n={source.n}
        data-variant={variant}
        onMouseEnter={show}
        onMouseLeave={() => {
          if (!pinned.current) hide();
        }}
        onFocus={show}
        onBlur={hide}
        onClick={() => {
          if (pinned.current) hide();
          else {
            show();
            pinned.current = true;
          }
        }}
        aria-expanded={open}
        aria-describedby={open ? popoverId : undefined}
        // The list entry's visible text (number and document) is its name. The badge says more than its number.
        aria-label={variant === 'badge' ? `Source ${source.n}: ${sourceLabel(source)}` : undefined}
        className={cn(
          variant === 'badge'
            ? 'mx-0.5 inline-flex h-5 min-w-5 items-center justify-center rounded-md bg-teal-subtle px-1 align-baseline text-[11px] font-bold text-teal transition-colors hover:bg-teal-light'
            : 'inline-flex min-h-8 max-w-full items-center gap-2 rounded-lg border border-border bg-bg-secondary px-2 py-1 text-left text-xs text-text-secondary transition-colors hover:bg-surface-hover',
          highlighted && (variant === 'badge' ? 'bg-teal-light ring-2 ring-teal' : 'border-teal bg-teal-subtle text-text-primary ring-1 ring-teal'),
        )}
      >
        {content}
      </button>
      {open &&
        createPortal(
          <div
            id={popoverId}
            role="tooltip"
            style={{ position: 'fixed', left: position.left, width: position.width, top: position.top, bottom: position.bottom, maxHeight: position.maxHeight }}
            className="z-[70] overflow-hidden rounded-xl border border-border bg-surface p-3 text-left shadow-lg"
          >
            <p className="mb-1.5 break-words text-xs font-semibold text-teal">
              {source.documentName}
              {source.page !== undefined && <span className="font-normal text-text-muted"> · Page {source.page}</span>}
            </p>
            {/* Plain text: React escapes it, and `whitespace-pre-wrap` keeps the line breaks. */}
            <p className="line-clamp-[16] whitespace-pre-wrap break-words text-xs leading-relaxed text-text-secondary">{source.snippet}</p>
          </div>,
          document.body,
        )}
    </>
  );
}

/** The compact sources list under an answer. */
export function SourcesList({ sources }: { sources: Source[] }) {
  return (
    <div className="mt-3 border-t border-border-subtle pt-2.5" data-testid="sources">
      <p className="mb-1.5 text-xs font-medium text-text-muted">
        Sources
      </p>
      <ul className="flex flex-wrap gap-1.5" aria-label="Sources for this answer">
        {sources.map((source) => (
          <li key={source.n} className="min-w-0 max-w-full">
            <SourceButton source={source} variant="entry" />
          </li>
        ))}
      </ul>
    </div>
  );
}
