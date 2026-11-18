import { useLocation, useSearchParams } from 'react-router';
import { safeNextPath } from '@/lib/format';

/**
 * `next` travels in the query string (it is a path, never personal data). The email travels in
 * router state, so it never lands in a URL, a Referer header, or a server log.
 */
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

/** Router state that carries the email to the next auth page. */
export function emailState(email: string) {
  return { email: email.trim() };
}
