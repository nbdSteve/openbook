import { createContext, useContext } from 'react';
import type { ModelInfo } from '@/services/api/types';

export interface ModelsContextValue {
  /** The catalog in display order. Empty when it could not be loaded. */
  models: ModelInfo[];
  find: (id: string | undefined) => ModelInfo | undefined;
  /** The deployment's default chat model, when known. */
  defaultModel: ModelInfo | undefined;
}

export const ModelsContext = createContext<ModelsContextValue | null>(null);

export function useModels(): ModelsContextValue {
  const value = useContext(ModelsContext);
  if (!value) throw new Error('useModels must be used inside ModelsProvider');
  return value;
}
