import { createContext, useContext } from 'react';

export interface SourceHighlight {
  active: number | null;
  onActive: (n: number | null) => void;
}

export const SourceHighlightContext = createContext<SourceHighlight>({ active: null, onActive: () => {} });

export function useSourceHighlight(): SourceHighlight {
  return useContext(SourceHighlightContext);
}
