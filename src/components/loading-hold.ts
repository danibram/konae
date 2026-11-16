import { useEffect, useRef, useState } from "react";

export const LOADING_DELAY_MS = 250;
export const LOADING_MINIMUM_MS = 400;

export function holdRemaining(startedAt: number, endedAt: number, delay = LOADING_DELAY_MS, minimum = LOADING_MINIMUM_MS): number {
  const elapsed = endedAt - startedAt;
  if (elapsed <= delay) return 0;
  return Math.max(0, delay + minimum - elapsed);
}

export function useLoadingHold(pending: boolean, { sinceNavigation = false }: { sinceNavigation?: boolean } = {}): boolean {
  const startedAt = useRef<number | null>(null);
  const holdUntil = useRef(0);
  const [, release] = useState(0);
  if (pending) {
    if (startedAt.current === null) startedAt.current = sinceNavigation ? 0 : performance.now();
    holdUntil.current = 0;
  } else if (startedAt.current !== null) {
    const now = performance.now();
    const remaining = holdRemaining(startedAt.current, now);
    startedAt.current = null;
    holdUntil.current = remaining > 0 ? now + remaining : 0;
  }
  const holding = !pending && holdUntil.current > performance.now();
  useEffect(() => {
    if (!holding) return;
    const timer = window.setTimeout(() => { holdUntil.current = 0; release((count) => count + 1); }, Math.max(0, holdUntil.current - performance.now()));
    return () => window.clearTimeout(timer);
  }, [holding]);
  return pending || holding;
}
