import { Link } from 'react-router';
import { cn } from '@/lib/cn';

/** The "Ask" wordmark. Links home. */
export function Logo({ className, to = '/' }: { className?: string; to?: string }) {
  return (
    <Link
      to={to}
      className={cn('inline-flex items-center gap-2 rounded-lg text-xl font-bold text-accent', className)}
      aria-label="Ask home"
    >
      <img src="/ask-logo.webp" alt="" className="h-7 w-7 rounded-full object-cover" />
      <span>Ask</span>
    </Link>
  );
}
