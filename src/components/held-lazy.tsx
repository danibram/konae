import { useEffect, useRef, useState, type ComponentType, type ReactNode } from "react";
import { holdRemaining, LOADING_MINIMUM_MS } from "./loading-hold";

export type HeldLazy<P> = ComponentType<P & { fallback: ReactNode }> & { preload: () => Promise<unknown> };

export function heldLazy<P extends object>(load: () => Promise<ComponentType<P>>): HeldLazy<P> {
  let loaded: ComponentType<P> | null = null;
  let pending: Promise<ComponentType<P>> | null = null;
  const preload = () => pending ??= load().then((component) => (loaded = component), (error: unknown) => { pending = null; throw error; });

  function Held({ fallback, ...props }: P & { fallback: ReactNode }) {
    const [shown, setShown] = useState(() => loaded !== null);
    const [failure, setFailure] = useState<{ error: unknown } | null>(null);
    const startedAt = useRef(performance.now());
    const holder = useRef<HTMLDivElement>(null);
    useEffect(() => {
      if (shown) return;
      let current = true;
      let timer = 0;
      preload().then(() => {
        if (!current) return;
        const remaining = remainingHold(holder.current, startedAt.current, performance.now());
        if (remaining > 0) timer = window.setTimeout(() => setShown(true), remaining);
        else setShown(true);
      }, (error: unknown) => { if (current) setFailure({ error }); });
      return () => { current = false; window.clearTimeout(timer); };
    }, [shown]);
    if (failure) throw failure.error;
    if (!shown || !loaded) return <div ref={holder} style={{ display: "contents" }}>{fallback}</div>;
    const Component = loaded;
    return <Component {...(props as P)} />;
  }
  return Object.assign(Held, { preload });
}

function remainingHold(holder: HTMLElement | null, renderedAt: number, now: number): number {
  const fade = holder?.getAnimations?.({ subtree: true }).find((animation) => animation.startTime !== null);
  if (!fade) return holdRemaining(renderedAt, now);
  const delay = Number(fade.effect?.getTiming().delay ?? 0);
  const visibleFrom = Number(fade.startTime) + delay;
  if (now <= visibleFrom) return 0;
  return Math.max(0, visibleFrom + LOADING_MINIMUM_MS - now);
}
