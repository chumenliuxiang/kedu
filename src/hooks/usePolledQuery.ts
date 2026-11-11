import { useCallback, useEffect, useRef, useState } from "react";

/** A query owns one request at a time, including across filter changes. */
export function usePolledQuery<T>(query: () => Promise<T>, initialValue: () => T, {
  intervalMs = 0,
  enabled = true,
  refreshKey = 0,
}: { intervalMs?: number; enabled?: boolean; refreshKey?: number } = {}) {
  const [data, setData] = useState(initialValue);
  const [state, setState] = useState<"loading" | "ready" | "error">("loading");
  const [error, setError] = useState("");
  const [revision, setRevision] = useState(0);
  const pending = useRef<Promise<void> | null>(null);
  const reload = useCallback(() => setRevision(value => value + 1), []);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    let running = false;
    let timer: number | undefined;
    setState("loading");
    setError("");

    const run = async () => {
      if (cancelled || running || document.hidden) return;
      running = true;
      window.clearTimeout(timer);
      // IPC cannot be cancelled. Wait for the previous filter's request, discard
      // its result, and issue only the most recently selected query afterwards.
      if (pending.current) await pending.current;
      if (cancelled || document.hidden) { running = false; return; }
      const request = Promise.resolve().then(query).then(value => {
        if (!cancelled && !document.hidden) {
          setData(value);
          setState("ready");
          setError("");
        }
      }).catch(reason => {
        if (!cancelled && !document.hidden) {
          setState("error");
          setError(String(reason));
        }
      });
      pending.current = request;
      await request;
      if (pending.current === request) pending.current = null;
      running = false;
      if (!cancelled && !document.hidden && intervalMs > 0) {
        timer = window.setTimeout(() => void run(), intervalMs);
      }
    };
    const onVisibility = () => {
      window.clearTimeout(timer);
      if (!document.hidden) void run();
    };
    document.addEventListener("visibilitychange", onVisibility);
    void run();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [query, intervalMs, enabled, refreshKey, revision]);

  return { data, state, error, reload };
}
