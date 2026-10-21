import { useEffect, useRef, useState } from "react";

/** A loader that would show for less than this stays hidden. */
export const LOADING_DELAY_MS = 200;
/** Once a loader has shown, it stays at least this long. */
export const LOADING_MINIMUM_MS = 300;

/**
 * How much longer a loader must stay up after its work finished.
 * Work that ended before the delay never showed a loader, so nothing is held.
 */
export function holdRemaining(startedAt: number, endedAt: number, delay = LOADING_DELAY_MS, minimum = LOADING_MINIMUM_MS): number {
  const elapsed = endedAt - startedAt;
  if (elapsed <= delay) return 0;
  return Math.max(0, delay + minimum - elapsed);
}

/**
 * True while `pending` is true, and for as long after as the loader needs to
 * reach its minimum time on screen. `sinceNavigation` counts from the page
 * load, for the boot stages that remount the same skeleton one after another.
 */
export function useLoadingHold(pending: boolean, { sinceNavigation = false }: { sinceNavigation?: boolean } = {}): boolean {
  const startedAt = useRef<number | null>(null);
  if (pending && startedAt.current === null) startedAt.current = sinceNavigation ? 0 : performance.now();
  const [holding, setHolding] = useState(false);
  useEffect(() => {
    if (pending) {
      setHolding(false);
      return;
    }
    if (startedAt.current === null) return;
    const remaining = holdRemaining(startedAt.current, performance.now());
    startedAt.current = null;
    if (remaining <= 0) return;
    setHolding(true);
    const timer = window.setTimeout(() => setHolding(false), remaining);
    return () => window.clearTimeout(timer);
  }, [pending]);
  return pending || holding;
}
