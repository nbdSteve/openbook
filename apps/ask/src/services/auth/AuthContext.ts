import { createContext, useContext } from 'react';
import type { SessionUser } from './cognito';

export interface AuthContextValue {
  status: 'loading' | 'signedOut' | 'signedIn';
  user: SessionUser | null;
  signIn: (email: string, password: string) => Promise<void>;
  signOut: () => Promise<void>;
}

export const AuthContext = createContext<AuthContextValue | null>(null);

export function useAuth(): AuthContextValue {
  const value = useContext(AuthContext);
  if (!value) throw new Error('useAuth must be used inside AuthProvider');
  return value;
}
