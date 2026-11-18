import { createContext, useContext } from 'react';
import type { ModelInfo } from '@/services/api/types';

export interface ModelsContextValue {
  models: ModelInfo[];
  find: (id: string | undefined) => ModelInfo | undefined;
  defaultModel: ModelInfo | undefined;
}

export const ModelsContext = createContext<ModelsContextValue | null>(null);

export function useModels(): ModelsContextValue {
  const value = useContext(ModelsContext);
  if (!value) throw new Error('useModels must be used inside ModelsProvider');
  return value;
}
