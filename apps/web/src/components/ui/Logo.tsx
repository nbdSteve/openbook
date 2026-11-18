import { Link } from 'react-router';
import { cn } from '@/lib/cn';
import { Mascot } from './Mascot';

export function Logo({ className, to = '/' }: { className?: string; to?: string }) {
  return (
    <Link
      to={to}
      className={cn('inline-flex shrink-0 items-center gap-2 whitespace-nowrap rounded-lg font-serif text-2xl font-semibold tracking-tight text-text-primary', className)}
      aria-label="Openbook home"
    >
      <Mascot size={34} />
      <span>Openbook</span>
    </Link>
  );
}
