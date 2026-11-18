import { useCallback, useEffect, useEffectEvent, useState } from 'react';

interface Result<T> {
  key: string;
  data?: T;
  error?: unknown;
}

export interface Resource<T> {
  data: T | undefined;
  error: unknown;
  /** True until the first result for the current key arrives. Reloads do not set it. */
  loading: boolean;
  reload: () => void;
  update: (updater: (current: T) => T) => void;
}

/**
 * Load data when `key` changes (and when `reload` runs). Stale results for an old key are never
 * returned, and requests for an old key are aborted.
 */
export function useResource<T>(key: string, load: (signal: AbortSignal) => Promise<T>): Resource<T> {
  const [result, setResult] = useState<Result<T>>();
  const [version, setVersion] = useState(0);
  const run = useEffectEvent(load);

  useEffect(() => {
    const controller = new AbortController();
    run(controller.signal).then(
      (data) => {
        if (!controller.signal.aborted) setResult({ key, data });
      },
      (error: unknown) => {
        if (!controller.signal.aborted) setResult((prev) => ({ key, data: prev?.key === key ? prev.data : undefined, error }));
      },
    );
    return () => controller.abort();
  }, [key, version]);

  const reload = useCallback(() => setVersion((v) => v + 1), []);
  const update = useCallback((updater: (current: T) => T) => {
    setResult((prev) => (prev && prev.data !== undefined ? { ...prev, data: updater(prev.data) } : prev));
  }, []);

  const current = result?.key === key ? result : undefined;
  return { data: current?.data, error: current?.error, loading: current === undefined, reload, update };
}
