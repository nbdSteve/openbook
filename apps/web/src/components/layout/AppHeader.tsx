import { LogOut } from 'lucide-react';
import { Logo } from '@/components/ui/Logo';
import { ThemeToggle } from '@/components/ui/ThemeToggle';
import { useAuth } from '@/services/auth/AuthContext';
import { AreaSwitch } from './AreaSwitch';

export function AppHeader() {
  const { user, signOut } = useAuth();
  return (
    <header className="sticky top-0 z-20 border-b border-border bg-bg-primary/90 backdrop-blur">
      <div className="mx-auto flex h-16 max-w-5xl items-center justify-between gap-2 px-4 sm:gap-3 sm:px-6">
        <div className="flex min-w-0 items-center gap-3 sm:gap-5">
          <Logo to="/ask" className="[&_span]:hidden sm:[&_span]:inline" />
          <AreaSwitch />
        </div>
        <div className="flex min-w-0 items-center gap-1">
          <span className="hidden min-w-0 max-w-[16rem] truncate px-2 text-sm text-text-secondary lg:block" title={user?.email}>
            {user?.email}
          </span>
          <ThemeToggle />
          <button
            type="button"
            onClick={() => {
              void signOut();
            }}
            className="inline-flex h-10 items-center gap-2 rounded-xl px-2.5 text-sm text-text-secondary hover:bg-surface-hover hover:text-text-primary sm:px-3"
          >
            <LogOut className="h-4 w-4" aria-hidden="true" />
            <span className="hidden sm:inline">Sign out</span>
            <span className="sr-only sm:hidden">Sign out</span>
          </button>
        </div>
      </div>
    </header>
  );
}
