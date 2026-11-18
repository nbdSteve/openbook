import { useLocation, useSearchParams } from 'react-router';
import { safeNextPath } from '@/lib/format';

export function useAuthParams() {
  const [params] = useSearchParams();
  const location = useLocation();
  const state = location.state as { email?: unknown } | null;
  return {
    email: typeof state?.email === 'string' ? state.email : '',
    next: params.get('next'),
    notice: params.get('notice'),
    safeNext: safeNextPath(params.get('next')),
  };
}

export function authPath(path: string, values: { next?: string | null; notice?: string } = {}): string {
  const params = new URLSearchParams();
  if (values.next) params.set('next', values.next);
  if (values.notice) params.set('notice', values.notice);
  const query = params.toString();
  return query ? `${path}?${query}` : path;
}

export function emailState(email: string) {
  return { email: email.trim() };
}
