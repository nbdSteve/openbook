import { Link } from 'react-router';
import { Compass } from 'lucide-react';
import { buttonStyles } from '@/components/ui/buttonStyles';
import { Logo } from '@/components/ui/Logo';
import { ThemeToggle } from '@/components/ui/ThemeToggle';

interface NotFoundScreenProps {
  title: string;
  message: string;
  actionTo: string;
  actionLabel: string;
}

export function NotFoundScreen({ title, message, actionTo, actionLabel }: NotFoundScreenProps) {
  return (
    <div className="relative flex min-h-dvh flex-col items-center justify-center bg-bg-primary px-6 text-center">
      <div className="absolute left-4 top-3">
        <Logo />
      </div>
      <div className="absolute right-4 top-4">
        <ThemeToggle />
      </div>
      <div className="mb-5 flex h-14 w-14 items-center justify-center rounded-2xl bg-accent-subtle">
        <Compass className="h-7 w-7 text-accent" aria-hidden="true" />
      </div>
      <h1 className="text-2xl font-bold text-text-primary">{title}</h1>
      <p className="mt-3 max-w-md text-text-secondary">{message}</p>
      <Link to={actionTo} className={buttonStyles('primary', 'md', 'mt-8')}>
        {actionLabel}
      </Link>
    </div>
  );
}
