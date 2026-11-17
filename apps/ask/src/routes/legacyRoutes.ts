import type { Params } from 'react-router';

/**
 * The routes before Openbook had an Ask area and a Portal area. Links in old emails, bookmarks, and
 * the browser history keep working: each one sends the visitor to the new address.
 */
export const LEGACY_ROUTES: { path: string; to: (params: Params) => string }[] = [
  { path: '/login', to: () => '/signin' },
  { path: '/app', to: () => '/ask' },
  { path: '/app/c/:courseId', to: (p) => `/ask/courses/${p.courseId}` },
  { path: '/app/c/:courseId/chat/:conversationId', to: (p) => `/ask/courses/${p.courseId}/c/${p.conversationId}` },
  { path: '/app/c/:courseId/manage', to: (p) => `/portal/courses/${p.courseId}` },
  // Any other old path below a course or below /app lands on the nearest page that exists.
  { path: '/app/c/:courseId/*', to: (p) => `/ask/courses/${p.courseId}` },
  { path: '/app/*', to: () => '/ask' },
];
