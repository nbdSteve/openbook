import { createContext, useContext } from 'react';
import type { Me } from '@/services/api/types';

export interface MeContextValue {
  me: Me;
  refresh: () => void;
}

export const MeContext = createContext<MeContextValue | null>(null);

export function useMe(): MeContextValue {
  const value = useContext(MeContext);
  if (!value) throw new Error('useMe must be used inside MeProvider');
  return value;
}
