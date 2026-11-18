import { useEffect, useRef } from 'react';
import { useLocation } from 'react-router';

function pageKey(pathname: string): string {
  const parts = pathname.split('/');
  return parts[2] === 'courses' ? parts.slice(0, 4).join('/') : pathname;
}

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
