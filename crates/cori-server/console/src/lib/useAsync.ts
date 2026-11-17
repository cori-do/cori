import { useCallback, useEffect, useRef, useState } from "react";
import { errMsg, useToken } from "./api";

export type Async<T> = { data?: T; error?: string; loading: boolean; reload: () => void };

/** Loads `fn` when `deps` or the identity change; keeps the previous data while reloading so views do not flash. */
export function useAsync<T>(fn: () => Promise<T>, deps: unknown[]): Async<T> {
  const token = useToken();
  const [state, setState] = useState<{ data?: T; error?: string; loading: boolean }>({ loading: true });
  const [tick, setTick] = useState(0);
  const seq = useRef(0);
  useEffect(() => {
    const n = ++seq.current;
    setState((s) => ({ ...s, loading: true }));
    fn().then(
      (data) => { if (n === seq.current) setState({ data, loading: false }); },
      (e) => { if (n === seq.current) setState({ error: errMsg(e), loading: false }); },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, token, tick]);
  const reload = useCallback(() => setTick((t) => t + 1), []);
  return { ...state, reload };
}
