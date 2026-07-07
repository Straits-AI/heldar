import { useCallback, useEffect, useRef, useState } from "react";

export interface PollState<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
  /** Manually re-run the fetcher (e.g. after a mutation). */
  refresh: () => Promise<void>;
}

/**
 * Fetch `fetcher` immediately and then every `intervalMs` (0 = no polling).
 * Re-runs when any value in `deps` changes. Keeps the previous `data` visible
 * while refetching so the UI does not flicker.
 */
export function usePoll<T>(
  fetcher: () => Promise<T>,
  intervalMs: number,
  deps: ReadonlyArray<unknown> = [],
): PollState<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;
  const mounted = useRef(true);

  const run = useCallback(async () => {
    try {
      const result = await fetcherRef.current();
      if (!mounted.current) return;
      setData(result);
      setError(null);
    } catch (e) {
      if (!mounted.current) return;
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (mounted.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    setLoading(true);
    void run();
    let timer: ReturnType<typeof setInterval> | undefined;
    if (intervalMs > 0) timer = setInterval(() => void run(), intervalMs);
    return () => {
      mounted.current = false;
      if (timer) clearInterval(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [run, intervalMs, ...deps]);

  return { data, error, loading, refresh: run };
}
