import { useMemo, type ReactNode } from 'react';
import { useResource } from '@/lib/useResource';
import { listModels } from '@/services/api/routes';
import { ModelsContext, type ModelsContextValue } from './ModelsContext';

export function ModelsProvider({ children }: { children: ReactNode }) {
  const { data } = useResource('models', listModels);
  const value = useMemo<ModelsContextValue>(() => {
    const models = data ?? [];
    return {
      models,
      find: (id) => (id ? models.find((m) => m.id === id) : undefined),
      defaultModel: models.find((m) => m.isDefault) ?? models[0],
    };
  }, [data]);
  return <ModelsContext.Provider value={value}>{children}</ModelsContext.Provider>;
}
