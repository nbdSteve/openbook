import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { setTokenProvider, setUnauthenticatedHandler } from '@/services/api/client';
import { AuthContext, type AuthContextValue } from './AuthContext';
import * as cognito from './cognito';

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthContextValue['status']>('loading');
  const [user, setUser] = useState<cognito.SessionUser | null>(null);
  const [endedBy, setEndedBy] = useState<AuthContextValue['endedBy']>(null);
  const timer = useRef<number | undefined>(undefined);

  const applySignedOut = useCallback((by: 'user' | 'expired' = 'expired') => {
    window.clearTimeout(timer.current);
    setUser(null);
    setEndedBy(by);
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
        else applySignedOut('expired');
      }, seconds * 1000);
    };
  }, [applySignedOut]);
  const scheduleRefresh = useCallback((expiresAt: number) => schedule.current(expiresAt), []);

  useEffect(() => {
    setTokenProvider(cognito.getIdToken);
    setUnauthenticatedHandler(() => {
      cognito.clearSession();
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

    // Another tab signed in or out: follow it.
    const onStorage = (event: StorageEvent) => {
      if (event.key !== null && !event.key.startsWith('CognitoIdentityServiceProvider.')) return;
      cognito.restoreSession().then((restored) => {
        if (cancelled) return;
        if (restored) {
          setUser(restored.user);
          setStatus('signedIn');
          scheduleRefresh(restored.expiresAt);
        } else {
          applySignedOut();
        }
      });
    };
    window.addEventListener('storage', onStorage);
    return () => {
      cancelled = true;
      window.clearTimeout(timer.current);
      window.removeEventListener('storage', onStorage);
    };
  }, [applySignedOut, scheduleRefresh]);

  const signIn = useCallback(
    async (email: string, password: string) => {
      const signedIn = await cognito.signIn(email, password);
      const restored = await cognito.restoreSession();
      setUser(signedIn);
      setEndedBy(null);
      setStatus('signedIn');
      if (restored) scheduleRefresh(restored.expiresAt);
    },
    [scheduleRefresh],
  );

  const signOut = useCallback(async () => {
    await cognito.signOut();
    applySignedOut('user');
  }, [applySignedOut]);

  const value = useMemo(() => ({ status, user, endedBy, signIn, signOut }), [status, user, endedBy, signIn, signOut]);
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}
