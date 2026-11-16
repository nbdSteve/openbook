import { Check, Circle } from 'lucide-react';
import { clsx } from 'clsx';
import { PASSWORD_MIN_LENGTH } from '@/lib/constants';

const RULES = [
  { id: 'length', label: `At least ${PASSWORD_MIN_LENGTH} characters`, test: (p: string) => p.length >= PASSWORD_MIN_LENGTH },
  { id: 'lower', label: 'A lower case letter', test: (p: string) => /[a-z]/.test(p) },
  { id: 'upper', label: 'An upper case letter', test: (p: string) => /[A-Z]/.test(p) },
  { id: 'number', label: 'A number', test: (p: string) => /\d/.test(p) },
  { id: 'symbol', label: 'A symbol, such as - or !', test: (p: string) => /[^A-Za-z0-9\s]/.test(p) },
];

export function PasswordRules({ password }: { password: string }) {
  return (
    <div className="rounded-xl bg-bg-secondary px-4 py-3 text-sm">
      <p className="mb-2 font-medium text-text-primary" id="password-rules-title">
        Your password needs
      </p>
      <ul aria-labelledby="password-rules-title" className="grid gap-1.5 sm:grid-cols-2">
        {RULES.map((rule) => {
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
