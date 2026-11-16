import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { setTokenProvider, setUnauthenticatedHandler } from '@/services/api/client';
import { AuthContext, type AuthContextValue } from './AuthContext';
import * as cognito from './cognito';

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthContextValue['status']>('loading');
  const [user, setUser] = useState<cognito.SessionUser | null>(null);
  const timer = useRef<number | undefined>(undefined);

  const applySignedOut = useCallback(() => {
    window.clearTimeout(timer.current);
    setUser(null);
    setStatus('signedOut');
  }, []);

  // Refresh the ID token shortly before it expires, even in a tab that sits idle.
  const schedule = useRef<(expiresAt: number) => void>(() => {});
  useEffect(() => {
    schedule.current = (expiresAt) => {
      window.clearTimeout(timer.current);
      const seconds = Math.max(30, expiresAt - Math.floor(Date.now() / 1000) - cognito.REFRESH_MARGIN_SECONDS);
      timer.current = window.setTimeout(async () => {
        const restored = await cognito.restoreSession();
        if (restored) schedule.current(restored.expiresAt);
        else applySignedOut();
      }, seconds * 1000);
    };
  }, [applySignedOut]);
  const scheduleRefresh = useCallback((expiresAt: number) => schedule.current(expiresAt), []);

  useEffect(() => {
    setTokenProvider(cognito.getIdToken);
    setUnauthenticatedHandler(() => {
      cognito.signOut();
      applySignedOut();
    });
    let cancelled = false;
    cognito.restoreSession().then((restored) => {
      if (cancelled) return;
      if (restored) {
        setUser(restored.user);
        setStatus('signedIn');
        scheduleRefresh(restored.expiresAt);
      } else {
        setStatus('signedOut');
      }
    });
    return () => {
      cancelled = true;
      window.clearTimeout(timer.current);
    };
  }, [applySignedOut, scheduleRefresh]);

  const signIn = useCallback(
    async (email: string, password: string) => {
      const signedIn = await cognito.signIn(email, password);
      const restored = await cognito.restoreSession();
      setUser(signedIn);
      setStatus('signedIn');
      if (restored) scheduleRefresh(restored.expiresAt);
    },
    [scheduleRefresh],
  );

  const signOut = useCallback(() => {
    cognito.signOut();
    applySignedOut();
  }, [applySignedOut]);

  const value = useMemo(() => ({ status, user, signIn, signOut }), [status, user, signIn, signOut]);
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}
