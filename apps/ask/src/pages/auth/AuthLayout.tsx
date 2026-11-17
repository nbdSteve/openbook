import type { ReactNode } from 'react';
import { Logo } from '@/components/ui/Logo';
import { ThemeToggle } from '@/components/ui/ThemeToggle';

export function AuthLayout({ title, subtitle, children, footer }: { title: string; subtitle?: ReactNode; children: ReactNode; footer?: ReactNode }) {
  return (
    <div className="flex min-h-dvh flex-col bg-bg-primary">
      <header className="flex h-16 items-center justify-between px-4 sm:px-6">
        <Logo />
        <ThemeToggle />
      </header>
      <main className="flex flex-1 items-start justify-center px-4 pb-12 pt-4 sm:items-center sm:pt-0">
        <div className="w-full max-w-md">
          <div className="rounded-2xl border border-border bg-surface p-6 shadow-lg sm:p-8">
            <h1 className="font-serif text-3xl font-medium text-text-primary">{title}</h1>
            {subtitle && <p className="mt-2 text-sm text-text-secondary">{subtitle}</p>}
            <div className="mt-6">{children}</div>
          </div>
          {footer && <div className="mt-5 text-center text-sm text-text-secondary">{footer}</div>}
        </div>
      </main>
    </div>
  );
}
