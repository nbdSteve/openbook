import { useEffect, useId, useRef, type ReactNode } from 'react';
import { X } from 'lucide-react';
import { Button } from './Button';

interface DialogProps {
  open: boolean;
  title: string;
  onClose: () => void;
  /** While true, Escape, the backdrop, and Close do nothing (a request is running). */
  locked?: boolean;
  children: ReactNode;
}

/** A modal on the native <dialog> element: focus trap, Escape to close, and backdrop. */
export function Dialog({ open, title, onClose, locked = false, children }: DialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const pressedBackdrop = useRef(false);
  const titleId = useId();

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (open && !el.open) {
      el.showModal();
      // Start on the first field, or on the control marked data-autofocus, not on Close.
      el.querySelector<HTMLElement>('[data-autofocus], input, textarea, select')?.focus();
    }
    if (!open && el.open) el.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      aria-labelledby={titleId}
      onClose={onClose}
      onCancel={(e) => {
        if (locked) e.preventDefault();
      }}
      onMouseDown={(e) => {
        pressedBackdrop.current = e.target === ref.current;
      }}
      onClick={(e) => {
        // Close only when the press and the release were both on the backdrop (not at the end of a text selection).
        if (e.target === ref.current && pressedBackdrop.current && !locked) onClose();
      }}
      className="m-auto w-[calc(100%-2rem)] max-w-md rounded-2xl border border-border bg-surface p-0 shadow-lg"
    >
      {open && (
        <div className="p-5 sm:p-6">
          <div className="mb-4 flex items-start justify-between gap-4">
            <h2 id={titleId} className="text-lg font-semibold text-text-primary">
              {title}
            </h2>
            <button
              type="button"
              onClick={onClose}
              disabled={locked}
              className="-mr-2 -mt-1 rounded-lg p-2.5 text-text-tertiary hover:bg-surface-hover hover:text-text-primary disabled:opacity-50"
              aria-label="Close"
            >
              <X className="h-4 w-4" aria-hidden="true" />
            </button>
          </div>
          {children}
        </div>
      )}
    </dialog>
  );
}

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  children: ReactNode;
  confirmLabel: string;
  danger?: boolean;
  busy?: boolean;
  /** Disable the confirm button until this is true (for typed confirmations). */
  canConfirm?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}

export function ConfirmDialog({
  open,
  title,
  children,
  confirmLabel,
  danger = false,
  busy = false,
  canConfirm = true,
  onConfirm,
  onClose,
}: ConfirmDialogProps) {
  return (
    <Dialog open={open} title={title} onClose={onClose} locked={busy}>
      <div className="space-y-4 text-sm text-text-secondary">{children}</div>
      <div className="mt-6 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
        <Button variant="secondary" onClick={onClose} disabled={busy} data-autofocus>
          Cancel
        </Button>
        <Button variant={danger ? 'danger' : 'primary'} onClick={onConfirm} isLoading={busy} disabled={!canConfirm}>
          {confirmLabel}
        </Button>
      </div>
    </Dialog>
  );
}
