import { LogOut } from 'lucide-react';
import { Logo } from '@/components/ui/Logo';
import { ThemeToggle } from '@/components/ui/ThemeToggle';
import { useAuth } from '@/services/auth/AuthContext';

/** The top bar for screens outside a course. */
export function AppHeader() {
  const { user, signOut } = useAuth();
  return (
    <header className="sticky top-0 z-20 border-b border-border bg-bg-primary/90 backdrop-blur">
      <div className="mx-auto flex h-16 max-w-5xl items-center justify-between gap-3 px-4 sm:px-6">
        <Logo to="/app" />
        <div className="flex min-w-0 items-center gap-1">
          <span className="hidden min-w-0 max-w-[16rem] truncate px-2 text-sm text-text-secondary sm:block" title={user?.email}>
            {user?.email}
          </span>
          <ThemeToggle />
          <button
            type="button"
            onClick={() => {
              void signOut();
            }}
            className="inline-flex h-10 items-center gap-2 rounded-xl px-3 text-sm text-text-secondary hover:bg-surface-hover hover:text-text-primary"
          >
            <LogOut className="h-4 w-4" aria-hidden="true" />
            Sign out
          </button>
        </div>
      </div>
    </header>
  );
}
