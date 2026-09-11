import { useCallback, useEffect, useRef, useState } from "react";
import { McpError, platformMcp } from "../mcp/client";

export type OwnerSession =
  | { state: "checking" }
  | { state: "signed-out" }
  | { state: "signed-in"; ownerId: string; email: string | null }
  /** The platform could not be reached, which is not the same as being signed out. */
  | { state: "unavailable"; message: string };

const emailKey = "littlebox:owner-email";

/** The email this browser last signed in with. The session check only returns an opaque ID. */
export function rememberOwnerEmail(email: string | null) {
  try {
    if (email) localStorage.setItem(emailKey, email);
    else localStorage.removeItem(emailKey);
  } catch {
    /* the account chip falls back to a generic label */
  }
}

export function isSignedOutError(error: unknown): boolean {
  if (error instanceof McpError && error.status === 401) return true;
  return error instanceof Error && /sign in to use/i.test(error.message);
}

export async function checkOwnerSession(): Promise<OwnerSession> {
  try {
    const result = await platformMcp.call<{ ownerId: string; displayName?: string | null }>("session_inspect");
    return { state: "signed-in", ownerId: result.ownerId, email: result.displayName ?? null };
  } catch (error) {
    if (isSignedOutError(error)) return { state: "signed-out" };
    return { state: "unavailable", message: error instanceof Error ? error.message : String(error) };
  }
}

/**
 * The owner's platform session, checked on load and again whenever the tab
 * regains focus, so signing in from another tab shows up without a reload.
 */
export function useOwnerSession(): [OwnerSession, () => void] {
  const [session, setSession] = useState<OwnerSession>({ state: "checking" });
  const lastCheck = useRef(0);
  const checkGeneration = useRef(0);
  const refresh = useCallback(() => {
    lastCheck.current = Date.now();
    const generation = ++checkGeneration.current;
    void checkOwnerSession().then((result) => {
      if (generation === checkGeneration.current) setSession(result);
    });
  }, []);

  useEffect(() => {
    refresh();
    // Focus also moves in and out of the preview iframe; one check every few seconds is plenty.
    const onFocus = () => {
      if (Date.now() - lastCheck.current > 5_000) refresh();
    };
    const onVisible = () => {
      if (document.visibilityState === "visible") onFocus();
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      checkGeneration.current++;
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [refresh]);

  return [session, refresh];
}

/** The in-app Platform access page, with a way back to where the owner came from. */
export function platformAccessHref(returnTo?: string): string {
  return returnTo ? `/platform-access?returnTo=${encodeURIComponent(returnTo)}` : "/platform-access";
}

export function ownerInitials(session: OwnerSession): string | null {
  if (session.state !== "signed-in" || !session.email) return null;
  const name = session.email.split("@")[0] ?? "";
  const parts = name.split(/[._-]+/).filter(Boolean);
  const letters = parts.length > 1 ? parts[0][0] + parts[1][0] : name.slice(0, 2);
  return letters.toUpperCase() || null;
}
