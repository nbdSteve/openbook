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
const POPOVER_MAX_HEIGHT = 320;

interface SourceButtonProps {
  source: Source;
  variant: 'badge' | 'entry' | 'other';
}

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
    const follow = (event: Event) => {
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
  }, [open]);

  useEffect(() => {
    if (isPinned && open) popover.current?.focus();
  }, [isPinned, open]);

  useEffect(() => () => window.clearTimeout(hideTimer.current), []);

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
        aria-label={variant === 'badge' ? `Source ${source.n}: ${sourceLabel(source)}` : undefined}
        className={cn(
          variant === 'badge'
            ? 'ml-[0.12em] inline-flex h-[1.45em] min-w-[1.45em] -translate-y-[0.4em] items-center justify-center rounded-full bg-accent-subtle px-[0.3em] align-baseline font-sans text-[0.68em] font-semibold text-accent transition-colors hover:bg-accent-light'
            : variant === 'entry'
              ? 'flex min-h-8 w-full items-start gap-2 rounded-r-lg border-l-2 border-accent/40 py-1.5 pl-3 pr-2 text-left text-[0.875rem] leading-snug text-text-secondary transition-colors hover:bg-accent-subtle'
              : 'flex min-h-8 w-full items-start gap-2 rounded-r-lg border-l-2 border-border py-1.5 pl-3 pr-2 text-left text-[0.8125rem] leading-snug text-text-muted transition-colors hover:bg-surface-hover',
          highlighted && (variant === 'badge' ? 'bg-accent-light ring-2 ring-accent' : 'bg-accent-subtle text-text-primary ring-1 ring-accent'),
        )}
      >
        {content}
      </button>
      {open &&
        createPortal(
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
  cited: Source[];
  other: Source[];
  streaming?: boolean;
}

export function SourcesList({ cited, other, streaming = false }: SourcesListProps) {
  const [expanded, setExpanded] = useState(false);
  const otherId = useId();
  const showOther = other.length > 0 && !streaming;
  if (cited.length === 0 && !showOther) return null;
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
