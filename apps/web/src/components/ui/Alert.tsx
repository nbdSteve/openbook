import type { ReactNode } from 'react';
import { clsx } from 'clsx';
import { AlertCircle, CheckCircle2, Info } from 'lucide-react';

interface AlertProps {
  tone?: 'error' | 'success' | 'info' | 'warning';
  children: ReactNode;
  className?: string;
}

const icons = { error: AlertCircle, warning: AlertCircle, success: CheckCircle2, info: Info };

/** Errors use role="alert" so screen readers announce them at once. */
export function Alert({ tone = 'info', children, className }: AlertProps) {
  const Icon = icons[tone];
  return (
    <div
      role={tone === 'error' ? 'alert' : 'status'}
      className={clsx(
        'flex items-start gap-3 rounded-xl border px-4 py-3 text-sm',
        tone === 'error' && 'border-danger/30 bg-danger-light text-danger',
        tone === 'warning' && 'border-warning/30 bg-warning-light text-warning',
        tone === 'success' && 'border-success/30 bg-success-light text-success',
        tone === 'info' && 'border-accent/20 bg-accent-subtle text-text-primary',
        className,
      )}
    >
      <Icon className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}
