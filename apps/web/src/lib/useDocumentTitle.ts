import { useEffect } from 'react';

/** Set the tab title to "<part> · Openbook". The product name always comes last. */
export function useDocumentTitle(part?: string): void {
  useEffect(() => {
    document.title = part ? `${part} · Openbook` : 'Openbook';
    return () => {
      document.title = 'Openbook';
    };
  }, [part]);
}
