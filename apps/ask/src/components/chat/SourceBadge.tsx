import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { ChevronRight } from 'lucide-react';
import { createPortal } from 'react-dom';
import { cn } from '@/lib/cn';
import type { Source } from '@/services/api/types';
import { chipName, sourceLabel } from './sourceText';
import { useSourceHighlight } from './SourceHighlight';

interface Position {
  left: number;
  width: number;
  top?: number;
  bottom?: number;
  maxHeight: number;
}

const POPOVER_WIDTH = 360;
/** The popover body scrolls past this height (in pixels), also when there is more room. */
const POPOVER_MAX_HEIGHT = 320;

interface SourceButtonProps {
  source: Source;
  /** `badge` is the inline [n] mark. `entry` is a cited source in the list. `other` is a passage that was found but not cited. */
  variant: 'badge' | 'entry' | 'other';
}

/**
 * A button that shows the source passage on hover or focus. A click, tap, or Enter pins the
 * popover open and moves focus into it, so a long passage can be scrolled with a pointer, a
 * finger, or the keyboard. Escape closes it and returns focus to the button. The passage is plain
 * text, never HTML or Markdown. The popover is fixed and clamped to the viewport. A badge and its
 * list entry share the highlight (through context), so hovering or focusing one lights the other.
 */
export function SourceButton({ source, variant }: SourceButtonProps) {
  const { active, onActive } = useSourceHighlight();
  const [position, setPosition] = useState<Position | null>(null);
  const pinned = useRef(false);
  const [isPinned, setIsPinned] = useState(false);
  const hideTimer = useRef<number | undefined>(undefined);
  const button = useRef<HTMLButtonElement>(null);
  const popover = useRef<HTMLDivElement>(null);
  const popoverId = useId();
  const open = position !== null;
  const highlighted = active === source.n;

  // True while this button is the one that set the highlight. Only then does it clear the highlight.
  const setsHighlight = useRef(false);

  const setPinned = (value: boolean) => {
    pinned.current = value;
    setIsPinned(value);
  };

  const show = () => {
    window.clearTimeout(hideTimer.current);
    setsHighlight.current = true;
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
        ? { left, width, bottom: window.innerHeight - rect.top + 8, maxHeight: Math.min(roomAbove, POPOVER_MAX_HEIGHT) }
        : { left, width, top: rect.bottom + 8, maxHeight: Math.min(roomBelow, POPOVER_MAX_HEIGHT) },
    );
  };
  const hide = () => {
    window.clearTimeout(hideTimer.current);
    setPinned(false);
    setsHighlight.current = false;
    onActive(null);
    setPosition(null);
  };
  // Leaving the button hides a hover popover after a short wait, so the pointer can cross the gap
  // into the popover (to scroll a long passage) without closing it.
  const hideSoon = () => {
    window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => {
      if (!pinned.current) hide();
    }, 150);
  };

  useEffect(() => {
    if (!open) return;
    const close = () => {
      setPinned(false);
      setsHighlight.current = false;
      onActive(null);
      setPosition(null);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      const focusInside = popover.current?.contains(document.activeElement);
      close();
      if (focusInside) button.current?.focus();
    };
    // Follow the button when the page scrolls or the layout shifts. Close if it leaves the screen.
    const follow = (event: Event) => {
      // Scrolling inside the popover is not a page scroll.
      if (event.target instanceof Node && popover.current?.contains(event.target)) return;
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

  // A pinned popover takes focus, so the keyboard can scroll it.
  useEffect(() => {
    if (isPinned && open) popover.current?.focus();
  }, [isPinned, open]);

  useEffect(() => () => window.clearTimeout(hideTimer.current), []);

  // A button that unmounts after it set the highlight must clear it, or the ring stays.
  const latest = useRef(onActive);
  useEffect(() => {
    latest.current = onActive;
  });
  useEffect(
    () => () => {
      if (setsHighlight.current) latest.current(null);
    },
    [],
  );

  const content: ReactNode =
    variant === 'badge' ? (
      source.n
    ) : (
      <>
        <span
          className={cn(
            'mt-px flex h-5 min-w-5 shrink-0 items-center justify-center rounded-full px-1 font-sans text-[11px] font-semibold',
            variant === 'other' ? 'bg-bg-tertiary text-text-muted' : 'bg-accent-subtle text-accent',
          )}
        >
          {source.n}
        </span>
        <span className="min-w-0 flex-1">
          <span className="line-clamp-2 break-words font-serif italic">{chipName(source.documentName)}</span>
          {source.page !== undefined && <span className="block font-sans text-[0.6875rem] not-italic text-text-muted">p. {source.page}</span>}
        </span>
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
          if (!pinned.current) hideSoon();
        }}
        onFocus={() => {
          if (!pinned.current) show();
        }}
        onBlur={() => {
          // Focus moving into a pinned popover is not a blur to act on.
          if (!pinned.current) hide();
        }}
        onClick={() => {
          if (pinned.current) hide();
          else {
            show();
            setPinned(true);
          }
        }}
        aria-expanded={open}
        aria-controls={open ? popoverId : undefined}
        // The list entry's visible text (number and document) is its name. The badge says more than its number.
        aria-label={variant === 'badge' ? `Source ${source.n}: ${sourceLabel(source)}` : undefined}
        className={cn(
          variant === 'badge'
            ? 'ml-[0.12em] inline-flex h-[1.45em] min-w-[1.45em] -translate-y-[0.4em] items-center justify-center rounded-full bg-accent-subtle px-[0.3em] align-baseline font-sans text-[0.68em] font-semibold text-accent transition-colors hover:bg-accent-light'
            : variant === 'entry'
              ? // A margin note: a rule on the left, the book in italics, and the page.
                'flex min-h-8 w-full items-start gap-2 rounded-r-lg border-l-2 border-accent/40 py-1.5 pl-3 pr-2 text-left text-[0.875rem] leading-snug text-text-secondary transition-colors hover:bg-accent-subtle'
              : // A passage that was found but not cited: no accent rule and muted text, so it does not look like evidence.
                'flex min-h-8 w-full items-start gap-2 rounded-r-lg border-l-2 border-border py-1.5 pl-3 pr-2 text-left text-[0.8125rem] leading-snug text-text-muted transition-colors hover:bg-surface-hover',
          highlighted && (variant === 'badge' ? 'bg-accent-light ring-2 ring-accent' : 'bg-accent-subtle text-text-primary ring-1 ring-accent'),
        )}
      >
        {content}
      </button>
      {open &&
        createPortal(
          // Hover: a tooltip. Pinned (click, tap, Enter): a labelled region that holds focus. Either way the body
          // scrolls and can take focus, so a long passage is reachable without a mouse.
          <div
            ref={popover}
            id={popoverId}
            role={isPinned ? 'dialog' : 'tooltip'}
            aria-label={isPinned ? `Source ${source.n}: ${sourceLabel(source)}` : undefined}
            tabIndex={0}
            onMouseEnter={() => window.clearTimeout(hideTimer.current)}
            onMouseLeave={() => {
              if (!pinned.current) hideSoon();
            }}
            onBlur={(event) => {
              // Focus left both the popover and its button: close.
              const next = event.relatedTarget as Node | null;
              if (isPinned && !popover.current?.contains(next) && next !== button.current) hide();
            }}
            style={{ position: 'fixed', left: position.left, width: position.width, top: position.top, bottom: position.bottom, maxHeight: position.maxHeight }}
            className="z-[70] overflow-y-auto overscroll-contain rounded-xl border border-border bg-surface p-3.5 text-left shadow-lg"
          >
            <p className="mb-1.5 break-words font-sans text-xs font-semibold text-accent">
              {source.documentName}
              {source.page !== undefined && <span className="font-normal text-text-muted"> · Page {source.page}</span>}
            </p>
            {/* Plain text: React escapes it, and `whitespace-pre-wrap` keeps the line breaks. */}
            <p data-testid="snippet" className="whitespace-pre-wrap break-words font-serif text-[0.9375rem] leading-relaxed text-text-secondary">
              {source.snippet}
            </p>
          </div>,
          document.body,
        )}
    </>
  );
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

interface SourcesListProps {
  /** The sources that the answer cites, in the order of the first citation. */
  cited: Source[];
  /** The other retrieved sources. They stay hidden until the student opens them. */
  other: Source[];
  /** While the answer streams, the set of cited sources is not final, so the other sources stay hidden. */
  streaming?: boolean;
}

/**
 * The compact sources list under an answer. It lists only the sources that the answer cites.
 * The other retrieved passages sit behind a disclosure, in a quieter style, so they do not look like evidence.
 */
export function SourcesList({ cited, other, streaming = false }: SourcesListProps) {
  const [expanded, setExpanded] = useState(false);
  const otherId = useId();
  const showOther = other.length > 0 && !streaming;
  if (cited.length === 0 && !showOther) return null;
  // The name stays the same in both states. The chevron and aria-expanded show the state.
  const label =
    cited.length === 0
      ? `${plural(other.length, 'passage', 'passages')} found, none cited`
      : `${plural(other.length, 'more passage', 'more passages')} found`;

  return (
    <div data-testid="sources">
      {cited.length > 0 && (
        <>
          <p className="mb-2 font-sans text-[0.6875rem] font-semibold uppercase tracking-[0.1em] text-text-muted">Sources</p>
          <ul role="list" className="flex flex-col gap-2" aria-label="Sources for this answer">
            {cited.map((source) => (
              <li key={source.n} className="min-w-0 max-w-full">
                <SourceButton source={source} variant="entry" />
              </li>
            ))}
          </ul>
        </>
      )}
      {showOther && (
        <>
          <button
            type="button"
            aria-expanded={expanded}
            aria-controls={otherId}
            data-testid="other-sources-toggle"
            onClick={() => setExpanded((value) => !value)}
            className={cn(
              '-ml-1 inline-flex min-h-8 items-start gap-1 rounded-lg px-1 py-2 text-left font-sans text-xs leading-4 text-text-muted transition-colors hover:text-text-secondary',
              cited.length > 0 && 'mt-1',
            )}
          >
            <ChevronRight className={cn('mt-px h-3.5 w-3.5 shrink-0 transition-transform', expanded && 'rotate-90')} aria-hidden="true" />
            <span>{label}</span>
          </button>
          <div id={otherId} hidden={!expanded}>
            {expanded && (
              <ul role="list" className="mt-1 flex flex-col gap-1" aria-label="Passages found">
                {other.map((source) => (
                  <li key={source.n} className="min-w-0 max-w-full">
                    <SourceButton source={source} variant="other" />
                  </li>
                ))}
              </ul>
            )}
          </div>
        </>
      )}
    </div>
  );
}
