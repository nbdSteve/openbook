import { lazy, Suspense, type ReactNode } from 'react';
import { BrowserRouter, Navigate, Outlet, Route, Routes, useLocation } from 'react-router';
import { Toaster } from 'react-hot-toast';
import { ErrorBoundary } from '@/components/layout/ErrorBoundary';
import { NotFoundScreen } from '@/components/layout/NotFoundScreen';
import { FullPageSpinner } from '@/components/ui/Spinner';
import { AuthProvider } from '@/services/auth/AuthProvider';
import { useAuth } from '@/services/auth/AuthContext';
import { MeProvider } from '@/services/me/MeProvider';
import { ModelsProvider } from '@/services/models/ModelsProvider';
import { ThemeProvider } from '@/theme/ThemeProvider';
import { ForgotPage } from '@/pages/auth/ForgotPage';
import { LoginPage } from '@/pages/auth/LoginPage';
import { SignupPage } from '@/pages/auth/SignupPage';
import { VerifyPage } from '@/pages/auth/VerifyPage';
import { JoinPage } from '@/pages/home/JoinPage';
import { authPath, useAuthParams } from '@/pages/auth/authParams';

const ChatPage = lazy(() => import('@/pages/course/ChatPage').then((m) => ({ default: m.ChatPage })));
const CourseLayout = lazy(() => import('@/pages/course/CourseLayout').then((m) => ({ default: m.CourseLayout })));
const ManagePage = lazy(() => import('@/pages/course/ManagePage').then((m) => ({ default: m.ManagePage })));
const CourseHome = lazy(() => import('@/pages/home/CourseHome').then((m) => ({ default: m.CourseHome })));
const LandingPage = lazy(() => import('@/pages/landing/LandingPage').then((m) => ({ default: m.LandingPage })));

/** Signed-in area: send signed-out visitors to sign in, and come back here after. */
function RequireAuth() {
  const { status } = useAuth();
  const location = useLocation();
  if (status === 'loading') return <FullPageSpinner label="Checking your session" />;
  if (status === 'signedOut') {
    return <Navigate to={authPath('/login', { next: location.pathname + location.search })} replace />;
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
function PublicOnly() {
  const { status } = useAuth();
  const { safeNext } = useAuthParams();
  if (status === 'loading') return <FullPageSpinner label="Checking your session" />;
  if (status === 'signedIn') return <Navigate to={safeNext} replace />;
  return <Outlet />;
}

function Home(): ReactNode {
  const { status } = useAuth();
  if (status === 'loading') return <FullPageSpinner label="Checking your session" />;
  if (status === 'signedIn') return <Navigate to="/app" replace />;
  return <LandingPage />;
}

export default function App() {
  return (
    <ThemeProvider>
      <ErrorBoundary>
        <AuthProvider>
          <BrowserRouter>
            <Suspense fallback={<FullPageSpinner />}>
              <Routes>
                <Route path="/" element={<Home />} />
                <Route element={<PublicOnly />}>
                  <Route path="/login" element={<LoginPage />} />
                  <Route path="/signup" element={<SignupPage />} />
                  <Route path="/verify" element={<VerifyPage />} />
                  <Route path="/forgot" element={<ForgotPage />} />
                </Route>
                <Route element={<RequireAuth />}>
                  <Route path="/join/:inviteCode" element={<JoinPage />} />
                  <Route path="/app" element={<CourseHome />} />
                  <Route path="/app/c/:courseId" element={<CourseLayout />}>
                    <Route index element={<ChatPage />} />
                    <Route path="chat/:conversationId" element={<ChatPage />} />
                    <Route path="manage" element={<ManagePage />} />
                  </Route>
                </Route>
                <Route
                  path="*"
                  element={
                    <NotFoundScreen
                      title="Page not found"
                      message="We could not find that page. Check the address, or go back to Ask."
                      actionTo="/"
                      actionLabel="Go to Ask"
                    />
                  }
                />
              </Routes>
            </Suspense>
          </BrowserRouter>
          <Toaster
            position="top-right"
            toastOptions={{
              style: {
                background: 'var(--color-surface)',
                color: 'var(--color-text-primary)',
                border: '1px solid var(--color-border)',
              },
            }}
          />
        </AuthProvider>
      </ErrorBoundary>
    </ThemeProvider>
  );
}
