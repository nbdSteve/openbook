import type { ReactNode } from 'react';
import { clsx } from 'clsx';

interface BadgeProps {
  tone?: 'neutral' | 'accent' | 'teal' | 'success' | 'warning' | 'danger';
  children: ReactNode;
  className?: string;
}

export function Badge({ tone = 'neutral', children, className }: BadgeProps) {
  return (
    <span
      className={clsx(
        'inline-flex items-center gap-1 whitespace-nowrap rounded-full px-2.5 py-0.5 text-xs font-medium',
        tone === 'neutral' && 'bg-bg-tertiary text-text-secondary',
        tone === 'accent' && 'bg-accent-subtle text-accent',
        tone === 'teal' && 'bg-teal-subtle text-teal',
        tone === 'success' && 'bg-success-light text-success',
        tone === 'warning' && 'bg-warning-light text-warning',
        tone === 'danger' && 'bg-danger-light text-danger',
        className,
      )}
    >
      {children}
    </span>
  );
}
