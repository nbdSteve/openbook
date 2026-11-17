import { Navigate, Outlet, useLocation } from 'react-router';
import { FullPageSpinner } from '@/components/ui/Spinner';
import { ShellSkeleton } from '@/components/layout/ShellSkeleton';
import { authPath, useAuthParams } from '@/pages/auth/authParams';
import { useAuth } from '@/services/auth/AuthContext';
import { MeProvider } from '@/services/me/MeProvider';
import { ModelsProvider } from '@/services/models/ModelsProvider';

/** Signed-in area: send signed-out visitors to sign in, and come back here after. */
export function RequireAuth() {
  const { status, endedBy } = useAuth();
  const location = useLocation();
  if (status === 'loading') return <ShellSkeleton label="Checking your session" />;
  if (status === 'signedOut') {
    // The user chose to sign out: show the landing page. Otherwise the session ended (it expired,
    // or another tab signed out): sign in again and come back here.
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

/** Sign-in pages: signed-in visitors go on to where they were headed. */
export function PublicOnly() {
  const { status } = useAuth();
  const { safeNext } = useAuthParams();
  if (status === 'loading') return <FullPageSpinner label="Checking your session" />;
  if (status === 'signedIn') return <Navigate to={safeNext} replace />;
  return <Outlet />;
}
