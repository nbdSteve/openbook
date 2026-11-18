import type { Params } from 'react-router';

export const LEGACY_ROUTES: { path: string; to: (params: Params) => string }[] = [
  { path: '/login', to: () => '/signin' },
  { path: '/app', to: () => '/ask' },
  { path: '/app/c/:courseId', to: (p) => `/ask/courses/${p.courseId}` },
  { path: '/app/c/:courseId/chat/:conversationId', to: (p) => `/ask/courses/${p.courseId}/c/${p.conversationId}` },
  { path: '/app/c/:courseId/manage', to: (p) => `/portal/courses/${p.courseId}` },
  { path: '/app/c/:courseId/*', to: (p) => `/ask/courses/${p.courseId}` },
  { path: '/app/*', to: () => '/ask' },
];
