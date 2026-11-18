import { Navigate, Outlet, useLocation } from 'react-router';
import { FullPageSpinner } from '@/components/ui/Spinner';
import { ShellSkeleton } from '@/components/layout/ShellSkeleton';
import { authPath, useAuthParams } from '@/pages/auth/authParams';
import { useAuth } from '@/services/auth/AuthContext';
import { MeProvider } from '@/services/me/MeProvider';
import { ModelsProvider } from '@/services/models/ModelsProvider';

export function RequireAuth() {
  const { status, endedBy } = useAuth();
  const location = useLocation();
  if (status === 'loading') return <ShellSkeleton label="Checking your session" />;
  if (status === 'signedOut') {
    if (endedBy === 'user') return <Navigate to="/" replace />;
    return <Navigate to={authPath('/signin', { next: location.pathname + location.search })} replace />;
  }
  return (
    <MeProvider>
      <ModelsProvider>
        <Outlet />
      </ModelsProvider>
    </MeProvider>
  );
}

export function PublicOnly() {
  const { status } = useAuth();
  const { safeNext } = useAuthParams();
  if (status === 'loading') return <FullPageSpinner label="Checking your session" />;
  if (status === 'signedIn') return <Navigate to={safeNext} replace />;
  return <Outlet />;
}
