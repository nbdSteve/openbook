import { useId } from 'react';
import { Check, Circle } from 'lucide-react';
import { clsx } from 'clsx';
import { PASSWORD_RULES } from '@/lib/password';

export function PasswordRules({ password }: { password: string }) {
  const titleId = useId();
  return (
    <div className="rounded-xl bg-bg-secondary px-4 py-3 text-sm">
      <p className="mb-2 font-medium text-text-primary" id={titleId}>
        Your password needs
      </p>
      <ul aria-labelledby={titleId} className="grid gap-1.5 sm:grid-cols-2">
        {PASSWORD_RULES.map((rule) => {
          const met = rule.test(password);
          return (
            <li key={rule.id} className={clsx('flex items-center gap-2', met ? 'text-success' : 'text-text-secondary')}>
              {met ? <Check className="h-4 w-4 shrink-0" aria-hidden="true" /> : <Circle className="h-3 w-3 shrink-0" aria-hidden="true" />}
              <span>
                {rule.label}
                <span className="sr-only">{met ? ' (done)' : ' (not done yet)'}</span>
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
