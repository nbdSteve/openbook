import { useEffect, useId, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { Source } from '@/services/api/types';

interface Position {
  left: number;
  width: number;
  top?: number;
  bottom?: number;
}

const POPOVER_WIDTH = 320;

/**
 * A `[n]` citation badge. The source passage shows on hover, focus, or tap (click toggles it).
 * The popover is fixed and clamped to the viewport, so it never causes horizontal overflow.
 */
export function SourceBadge({ source, label }: { source: Source; label?: string }) {
  const [position, setPosition] = useState<Position | null>(null);
  // A click or tap pins the popover open. Touch browsers send mouseenter before click.
  const pinned = useRef(false);
  const button = useRef<HTMLButtonElement>(null);
  const popoverId = useId();
  const open = position !== null;

  const show = () => {
    const rect = button.current?.getBoundingClientRect();
    if (!rect) return;
    const width = Math.min(POPOVER_WIDTH, window.innerWidth - 16);
    const left = Math.min(Math.max(8, rect.left + rect.width / 2 - width / 2), window.innerWidth - width - 8);
    const above = rect.top > 220;
    setPosition(
      above
        ? { left, width, bottom: window.innerHeight - rect.top + 8 }
        : { left, width, top: rect.bottom + 8 },
    );
  };
  const hide = () => {
    pinned.current = false;
    setPosition(null);
  };

  useEffect(() => {
    if (!open) return;
    const close = () => {
      pinned.current = false;
      setPosition(null);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
    };
    // Follow the badge when the page scrolls or the layout shifts. Close if it leaves the screen.
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
  }, [open]);

  return (
    <>
      <button
        ref={button}
        type="button"
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
        aria-label={`Source ${source.n}: ${source.documentName}`}
        className="mx-0.5 inline-flex h-5 min-w-5 items-center justify-center rounded-md bg-teal-subtle px-1 align-baseline text-[11px] font-bold text-teal transition-colors hover:bg-teal-light"
      >
        {label ?? source.n}
      </button>
      {open &&
        createPortal(
          <div
            id={popoverId}
            role="tooltip"
            style={{ position: 'fixed', left: position.left, width: position.width, top: position.top, bottom: position.bottom }}
            className="z-[70] rounded-xl border border-border bg-surface p-3 text-left shadow-lg"
          >
            <div className="mb-1.5 flex items-start justify-between gap-3">
              <span className="min-w-0 break-words text-xs font-semibold text-teal">{source.documentName}</span>
              {source.page !== undefined && <span className="shrink-0 text-xs text-text-muted">Page {source.page}</span>}
            </div>
            <p className="line-clamp-6 text-xs leading-relaxed text-text-secondary">{source.snippet}</p>
          </div>,
          document.body,
        )}
    </>
  );
}
