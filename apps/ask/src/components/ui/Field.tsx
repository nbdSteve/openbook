import { useId, type InputHTMLAttributes, type ReactNode, type SelectHTMLAttributes, type TextareaHTMLAttributes } from 'react';
import { clsx } from 'clsx';
import { ChevronDown } from 'lucide-react';

const control =
  'block w-full rounded-xl bg-surface border border-border-strong text-text-primary px-4 py-2.5 text-base sm:text-sm min-h-11 ' +
  'placeholder:text-text-muted transition-colors duration-150 ' +
  'focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent disabled:opacity-60';

interface FieldShellProps {
  id: string;
  label: string;
  hint?: ReactNode;
  error?: string;
  children: ReactNode;
}

function FieldShell({ id, label, hint, error, children }: FieldShellProps) {
  return (
    <div className="space-y-1.5">
      <label htmlFor={id} className="block text-sm font-medium text-text-primary">
        {label}
      </label>
      {children}
      {hint && (
        <div id={`${id}-hint`} className="text-xs text-text-tertiary">
          {hint}
        </div>
      )}
      {error && (
        <p id={`${id}-error`} className="text-sm text-danger" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function describedBy(id: string, hint: unknown, error: unknown) {
  const ids = [hint ? `${id}-hint` : '', error ? `${id}-error` : ''].filter(Boolean).join(' ');
  return ids || undefined;
}

interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  label: string;
  hint?: ReactNode;
  error?: string;
}

export function Input({ label, hint, error, className, ...props }: InputProps) {
  const id = useId();
  return (
    <FieldShell id={id} label={label} hint={hint} error={error}>
      <input
        id={id}
        className={clsx(control, error && 'border-danger', className)}
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy(id, hint, error)}
        {...props}
      />
    </FieldShell>
  );
}

interface TextareaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  label: string;
  hint?: ReactNode;
  error?: string;
}

export function Textarea({ label, hint, error, className, ...props }: TextareaProps) {
  const id = useId();
  return (
    <FieldShell id={id} label={label} hint={hint} error={error}>
      <textarea
        id={id}
        className={clsx(control, 'resize-y', error && 'border-danger', className)}
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy(id, hint, error)}
        {...props}
      />
    </FieldShell>
  );
}

interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  label: string;
  hint?: ReactNode;
  error?: string;
}

export function Select({ label, hint, error, className, children, ...props }: SelectProps) {
  const id = useId();
  return (
    <FieldShell id={id} label={label} hint={hint} error={error}>
      <div className="relative">
        <select
          id={id}
          className={clsx(control, 'cursor-pointer appearance-none pr-10', className)}
          aria-describedby={describedBy(id, hint, error)}
          {...props}
        >
          {children}
        </select>
        <ChevronDown className="pointer-events-none absolute right-3.5 top-1/2 h-4 w-4 -translate-y-1/2 text-text-muted" aria-hidden="true" />
      </div>
    </FieldShell>
  );
}

interface CheckboxProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'type'> {
  label: string;
  hint?: ReactNode;
}

export function Checkbox({ label, hint, className, ...props }: CheckboxProps) {
  const id = useId();
  return (
    <div className="flex items-start gap-3">
      <input
        id={id}
        type="checkbox"
        className={clsx('mt-0.5 h-5 w-5 shrink-0 cursor-pointer rounded border-border-strong accent-accent', className)}
        aria-describedby={hint ? `${id}-hint` : undefined}
        {...props}
      />
      <div className="space-y-1">
        <label htmlFor={id} className="block cursor-pointer text-sm font-medium text-text-primary">
          {label}
        </label>
        {hint && (
          <div id={`${id}-hint`} className="text-xs text-text-tertiary">
            {hint}
          </div>
        )}
      </div>
    </div>
  );
}
