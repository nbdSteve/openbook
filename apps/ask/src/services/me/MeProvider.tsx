import { useMemo, type ReactNode } from 'react';
import { useResource } from '@/lib/useResource';
import { getMe } from '@/services/api/routes';
import { describeApiError } from '@/services/api/errors';
import { Button } from '@/components/ui/Button';
import { ShellSkeleton } from '@/components/layout/ShellSkeleton';
import { MeContext } from './MeContext';

/** Loads `GET /api/me` once for the signed-in area. The first call also creates the profile. */
export function MeProvider({ children }: { children: ReactNode }) {
  const { data, error, loading, reload } = useResource('me', getMe);
  const value = useMemo(() => (data ? { me: data, refresh: reload } : null), [data, reload]);

  if (loading) return <ShellSkeleton label="Loading your account" />;
  if (!value) {
    return (
      <div className="flex min-h-dvh flex-col items-center justify-center gap-4 bg-bg-primary p-6 text-center">
        <h1 className="text-xl font-semibold text-text-primary">We could not load your account</h1>
        <p role="alert" className="max-w-md text-text-secondary">
          {describeApiError(error)}
        </p>
        <Button onClick={reload}>Try again</Button>
      </div>
    );
  }
  return <MeContext.Provider value={value}>{children}</MeContext.Provider>;
}
