import { lazy, Suspense, type ReactNode } from 'react';
import { BrowserRouter, Navigate, Route, Routes } from 'react-router';
import { Toaster } from 'react-hot-toast';
import { ErrorBoundary } from '@/components/layout/ErrorBoundary';
import { NotFoundScreen } from '@/components/layout/NotFoundScreen';
import { FullPageSpinner } from '@/components/ui/Spinner';
import { AuthProvider } from '@/services/auth/AuthProvider';
import { useAuth } from '@/services/auth/AuthContext';
import { PublicOnly, RequireAuth } from '@/routes/guards';
import { LegacyRedirect } from '@/routes/legacy';
import { RouteFocus } from '@/routes/RouteFocus';
import { LEGACY_ROUTES } from '@/routes/legacyRoutes';
import { ThemeProvider } from '@/theme/ThemeProvider';
import { ForgotPage } from '@/pages/auth/ForgotPage';
import { LoginPage } from '@/pages/auth/LoginPage';
import { SignupPage } from '@/pages/auth/SignupPage';
import { VerifyPage } from '@/pages/auth/VerifyPage';
import { JoinPage } from '@/pages/home/JoinPage';

const ChatPage = lazy(() => import('@/pages/course/ChatPage').then((m) => ({ default: m.ChatPage })));
const CourseLayout = lazy(() => import('@/pages/course/CourseLayout').then((m) => ({ default: m.CourseLayout })));
const ManagePage = lazy(() => import('@/pages/course/ManagePage').then((m) => ({ default: m.ManagePage })));
const AskHome = lazy(() => import('@/pages/home/AskHome').then((m) => ({ default: m.AskHome })));
const PortalHome = lazy(() => import('@/pages/home/PortalHome').then((m) => ({ default: m.PortalHome })));
const PortalCourseLayout = lazy(() => import('@/pages/course/PortalCourseLayout').then((m) => ({ default: m.PortalCourseLayout })));
const LandingPage = lazy(() => import('@/pages/landing/LandingPage').then((m) => ({ default: m.LandingPage })));

function Home(): ReactNode {
  const { status } = useAuth();
  if (status === 'loading') return <FullPageSpinner label="Checking your session" />;
  if (status === 'signedIn') return <Navigate to="/ask" replace />;
  return <LandingPage />;
}

export default function App() {
  return (
    <ThemeProvider>
      <ErrorBoundary>
        <AuthProvider>
          <BrowserRouter>
            <RouteFocus />
            <Suspense fallback={<FullPageSpinner />}>
              <Routes>
                <Route path="/" element={<Home />} />
                <Route element={<PublicOnly />}>
                  <Route path="/signin" element={<LoginPage />} />
                  <Route path="/signup" element={<SignupPage />} />
                  <Route path="/verify" element={<VerifyPage />} />
                  <Route path="/forgot" element={<ForgotPage />} />
                </Route>
                <Route element={<RequireAuth />}>
                  <Route path="/join/:inviteCode" element={<JoinPage />} />
                  <Route path="/ask" element={<AskHome />} />
                  <Route path="/ask/courses/:courseId" element={<CourseLayout />}>
                    <Route index element={<ChatPage />} />
                    <Route path="c/:conversationId" element={<ChatPage />} />
                  </Route>
                  <Route path="/portal" element={<PortalHome />} />
                  <Route path="/portal/courses/:courseId" element={<PortalCourseLayout />}>
                    <Route index element={<ManagePage />} />
                  </Route>
                </Route>
                {LEGACY_ROUTES.map((route) => (
                  <Route key={route.path} path={route.path} element={<LegacyRedirect to={route.to} />} />
                ))}
                <Route
                  path="*"
                  element={
                    <NotFoundScreen
                      title="Page not found"
                      message="We could not find that page. Check the address, or go back to Openbook."
                      actionTo="/"
                      actionLabel="Go to Openbook"
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
