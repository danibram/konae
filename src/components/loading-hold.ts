import { useEffect, useRef, useState } from "react";

/** A loader that would show for less than this stays hidden. */
export const LOADING_DELAY_MS = 250;
/** Once a loader has shown, it stays at least this long. */
export const LOADING_MINIMUM_MS = 400;

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
 *
 * The hold is decided in the same render that sees `pending` turn false, so
 * the loader never leaves the tree for a render and comes back. A remounted
 * loader would restart its fade from zero and blink out mid-hold.
 */
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
    // The timer may fire a fraction of a millisecond early, so it ends the hold itself instead of asking the clock again.
    const timer = window.setTimeout(() => { holdUntil.current = 0; release((count) => count + 1); }, Math.max(0, holdUntil.current - performance.now()));
    return () => window.clearTimeout(timer);
  }, [holding]);
  return pending || holding;
}
