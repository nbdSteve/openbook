import { cn } from '@/lib/cn';

export type ButtonVariant = 'primary' | 'secondary' | 'danger' | 'ghost';
export type ButtonSize = 'sm' | 'md' | 'lg';

/** Class names shared by <Button> and links that look like buttons. */
export function buttonStyles(variant: ButtonVariant = 'primary', size: ButtonSize = 'md', className?: string) {
  return cn(
    'inline-flex items-center justify-center gap-2 font-medium rounded-xl transition-colors duration-150 select-none',
    'disabled:opacity-50 disabled:cursor-not-allowed',
    {
      'bg-accent text-on-accent hover:bg-accent-hover shadow-sm': variant === 'primary',
      'bg-surface text-text-primary hover:bg-surface-hover border border-border': variant === 'secondary',
      'bg-danger-solid text-white hover:brightness-110': variant === 'danger',
      'text-text-secondary hover:bg-surface-hover hover:text-text-primary': variant === 'ghost',
    },
    {
      'px-3 py-1.5 text-sm min-h-9': size === 'sm',
      'px-4 py-2.5 text-sm min-h-11': size === 'md',
      'px-6 py-3 text-base min-h-12': size === 'lg',
    },
    className,
  );
}
