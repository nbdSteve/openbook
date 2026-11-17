import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { clsx } from 'clsx';
import { Check, ChevronDown, GraduationCap, Lock } from 'lucide-react';
import { styleName, TUTORING_STYLES } from '@/lib/constants';
import type { TutoringStyle } from '@/services/api/types';

interface StylePickerProps {
  value: TutoringStyle;
  onChange: (style: TutoringStyle) => void;
  disabled?: boolean;
  locked?: boolean;
  lockedReason?: string;
  placement?: 'up' | 'down';
  className?: string;
}

export function StylePicker({ value, onChange, disabled, locked, lockedReason, placement = 'up', className }: StylePickerProps) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const items = useRef<(HTMLButtonElement | null)[]>([]);
  const menuId = useId();
  const reasonId = useId();
  const selected = Math.max(
    0,
    TUTORING_STYLES.findIndex((style) => style.id === value),
  );

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointerDown);
    return () => document.removeEventListener('pointerdown', onPointerDown);
  }, [open]);
  const [offAt, setOffAt] = useState(false);
  const off = Boolean(disabled || locked);
  if (off !== offAt) {
    setOffAt(off);
    if (off) setOpen(false);
  }

  const menu = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = menu.current;
    if (!open || !element) return;
    element.style.transform = '';
    const overflow = element.getBoundingClientRect().right - (window.innerWidth - 12);
    if (overflow > 0) element.style.transform = `translateX(${-Math.ceil(overflow)}px)`;
  }, [open]);

  useEffect(() => {
    if (open) items.current[selected]?.focus();
  }, [open, selected]);

  const close = (refocus: boolean) => {
    setOpen(false);
    if (refocus) button.current?.focus();
  };

  const onMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const count = TUTORING_STYLES.length;
    const current = items.current.findIndex((item) => item === document.activeElement);
    let next: number;
    switch (event.key) {
      case 'ArrowDown':
        next = (current + 1) % count;
        break;
      case 'ArrowUp':
        next = (current - 1 + count) % count;
        break;
      case 'Home':
        next = 0;
        break;
      case 'End':
        next = count - 1;
        break;
      case 'Escape':
        event.preventDefault();
        event.stopPropagation();
        close(true);
        return;
      case 'Tab':
        setOpen(false);
        return;
      default:
        return;
    }
    event.preventDefault();
    items.current[next]?.focus();
  };

  const onButtonKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (locked || disabled) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      setOpen(true);
    }
  };

  const Icon = locked ? Lock : GraduationCap;

  return (
    <div ref={root} className={clsx('relative inline-flex min-w-0 max-w-full items-center gap-2', className)} data-testid="style-picker">
      <button
        ref={button}
        type="button"
        disabled={disabled}
        aria-haspopup={locked ? undefined : 'menu'}
        aria-expanded={locked ? undefined : open}
        aria-controls={open ? menuId : undefined}
        aria-disabled={locked || undefined}
        aria-describedby={locked && lockedReason ? reasonId : undefined}
        onClick={() => {
          if (locked) return;
          setOpen((was) => !was);
        }}
        onKeyDown={onButtonKeyDown}
        className={clsx(
          'inline-flex h-9 min-w-0 max-w-full items-center gap-1.5 rounded-lg border border-border-strong bg-surface px-2.5 text-xs font-medium text-text-secondary',
          'hover:bg-surface-hover disabled:cursor-not-allowed disabled:opacity-60',
          locked && 'cursor-default hover:bg-surface',
        )}
      >
        <Icon className="h-3.5 w-3.5 shrink-0 text-text-muted" aria-hidden="true" />
        <span className="sr-only">Tutoring style: </span>
        <span className="truncate">{styleName(value)}</span>
        {!locked && <ChevronDown className={clsx('h-3.5 w-3.5 shrink-0 text-text-muted transition-transform', open && 'rotate-180')} aria-hidden="true" />}
      </button>
      {locked && lockedReason && (
        <span id={reasonId} className="min-w-0 text-xs text-text-muted" data-testid="style-locked-reason">
          {lockedReason}
        </span>
      )}
      {open && !locked && (
        <div
          id={menuId}
          ref={menu}
          role="menu"
          aria-label="Tutoring style for your next message"
          onKeyDown={onMenuKeyDown}
          className={clsx(
            'absolute left-0 z-30 w-72 max-w-[calc(100vw-1.5rem)] rounded-xl border border-border bg-surface p-1.5 shadow-lg',
            placement === 'up' ? 'bottom-full mb-2' : 'top-full mt-2',
          )}
        >
          {TUTORING_STYLES.map((style, index) => {
            const checked = style.id === value;
            return (
              <button
                key={style.id}
                ref={(node) => {
                  items.current[index] = node;
                }}
                type="button"
                role="menuitemradio"
                aria-checked={checked}
                tabIndex={index === selected ? 0 : -1}
                onClick={() => {
                  onChange(style.id);
                  close(true);
                }}
                className={clsx(
                  'flex w-full items-start gap-2 rounded-lg px-2.5 py-2 text-left hover:bg-surface-hover focus-visible:bg-surface-hover',
                  checked && 'bg-accent-subtle',
                )}
              >
                <Check className={clsx('mt-0.5 h-3.5 w-3.5 shrink-0 text-accent', !checked && 'invisible')} aria-hidden="true" />
                <span className="min-w-0">
                  <span className="block text-sm font-medium text-text-primary">{style.name}</span>
                  <span className="block text-xs leading-snug text-text-tertiary">{style.description}</span>
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
