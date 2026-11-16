import { createContext, useContext } from 'react';

export interface SourceHighlight {
  /** The source number that is highlighted (by hover or focus on its badge or its list entry). */
  active: number | null;
  onActive: (n: number | null) => void;
}

/**
 * A badge in the text and its entry in the sources list highlight together. The state comes from
 * context, not from props, so the Markdown component map stays the same while it changes (a new
 * map would remount the badge and drop its focus).
 */
export const SourceHighlightContext = createContext<SourceHighlight>({ active: null, onActive: () => {} });

export function useSourceHighlight(): SourceHighlight {
  return useContext(SourceHighlightContext);
}
