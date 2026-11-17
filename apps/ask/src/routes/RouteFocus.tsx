import { useEffect, useRef } from 'react';
import { useLocation } from 'react-router';

/** The part of the path that names a page: the area and the course. A new chat in the same course keeps it. */
function pageKey(pathname: string): string {
  const parts = pathname.split('/');
  return parts[2] === 'courses' ? parts.slice(0, 4).join('/') : pathname;
}

/**
 * After the visitor moves to another page, move the focus to its h1. A single-page app does not
 * reload, so a keyboard or screen reader user would stay at the link they used. The pages load
 * lazily, so the h1 may arrive a moment later. The first page load, a new chat in the same course,
 * and a change of the query leave the focus alone.
 */
export function RouteFocus() {
  const { pathname } = useLocation();
  const previous = useRef(pageKey(pathname));
  useEffect(() => {
    const key = pageKey(pathname);
    if (key === previous.current) return;
    previous.current = key;
    let tries = 0;
    const timer = window.setInterval(() => {
      const heading = document.querySelector<HTMLElement>('h1');
      tries += 1;
      if (heading) {
        heading.tabIndex = -1;
        heading.focus({ preventScroll: true });
      }
      if (heading || tries >= 40) window.clearInterval(timer);
    }, 50);
    return () => window.clearInterval(timer);
  }, [pathname]);
  return null;
}
