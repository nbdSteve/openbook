import { createContext, useContext, type Dispatch, type SetStateAction } from 'react';

export interface SourceHighlight {
  active: number | null;
  onActive: Dispatch<SetStateAction<number | null>>;
}

export const SourceHighlightContext = createContext<SourceHighlight>({ active: null, onActive: () => {} });

export function useSourceHighlight(): SourceHighlight {
  return useContext(SourceHighlightContext);
}
