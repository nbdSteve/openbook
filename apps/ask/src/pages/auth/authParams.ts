import { useSearchParams } from 'react-router';
import { safeNextPath } from '@/lib/format';

/** `email` and `next` travel through the sign-in, sign-up, verify, and reset pages. */
export function useAuthParams() {
  const [params] = useSearchParams();
  return {
    email: params.get('email') ?? '',
    next: params.get('next'),
    notice: params.get('notice'),
    safeNext: safeNextPath(params.get('next')),
  };
}

export function authPath(path: string, values: { email?: string; next?: string | null; notice?: string }): string {
  const params = new URLSearchParams();
  if (values.email) params.set('email', values.email);
  if (values.next) params.set('next', values.next);
  if (values.notice) params.set('notice', values.notice);
  const query = params.toString();
  return query ? `${path}?${query}` : path;
}
