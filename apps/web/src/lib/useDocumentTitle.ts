import { useEffect } from 'react';

export function useDocumentTitle(part?: string): void {
  useEffect(() => {
    document.title = part ? `${part} · Openbook` : 'Openbook';
    return () => {
      document.title = 'Openbook';
    };
  }, [part]);
}
