import { Box } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useOwnerSession } from "./owner-session";
import { safeReturnPath } from "./return-path";
import { Card, LoadingBlock, Skeleton } from "./ui";

type ClerkBrowser = { load: (options: object) => Promise<void>; session: { getToken: () => Promise<string | null> } | null; mountSignIn: (element: HTMLElement) => void; unmountSignIn: (element: HTMLElement) => void; addListener: (listener: () => void) => () => void; signOut: () => Promise<void> };
declare global { interface Window { Clerk?: ClerkBrowser; __internal_ClerkUICtor?: unknown } }

/** The owner page the server serves, for sign-in, the account and agent tokens. */
export function ownerPageHref(returnTo: string | null): string {
  return returnTo ? `/_konae/owner/login?returnTo=${encodeURIComponent(returnTo)}` : "/_konae/owner/login";
}

/**
 * The editor's door to owner sign-in. The server's owner page at
 * `/_konae/owner/login` does sign-in, account creation, password links and
 * agent tokens for both the editor and an installation without it, so this
 * page sends the owner there. It only keeps the Clerk widget, which signs an
 * owner in on an installation that uses Clerk; once signed in, the owner is
 * sent on as well.
 */
export function PlatformAccess() {
  const params = useMemo(() => new URLSearchParams(window.location.search), []);
  const returnTo = useMemo(() => safeReturnPath(params.get("returnTo")), [params]);
  const signOut = params.has("signout");
  const [session] = useOwnerSession();
  const [config, setConfig] = useState<{ mode: "local" | "clerk"; publishableKey?: string | null } | null>(null);
  const [error, setError] = useState("");
  useEffect(() => { void fetch("/_konae/owner/config").then(async (response) => {
    if (!response.ok) throw new Error("Could not load sign-in settings.");
    setConfig(await response.json());
  }).catch((cause) => setError(cause instanceof Error ? cause.message : String(cause))); }, []);

  const toOwnerPage = config?.mode === "local" || (config?.mode === "clerk" && session.state === "signed-in" && !signOut);
  useEffect(() => { if (toOwnerPage) window.location.replace(ownerPageHref(returnTo)); }, [toOwnerPage, returnTo]);
  const finish = useCallback(() => window.location.replace(ownerPageHref(returnTo)), [returnTo]);

  const clerk = config?.mode === "clerk" && (session.state === "signed-out" || signOut);
  return (
    <div className="platform-page">
      <header className="platform-header">
        <a className="platform-brand" href="/"><span><Box size={16} /></span>konae</a>
      </header>
      <main className="platform-main">
        <Card className="platform-card">
          <h1>Owner sign-in</h1>
          <p className="platform-lead">Owners publish apps and manage their data. People who use your apps sign in inside each app, not here.</p>
          {error && <p className="platform-error" role="alert">Could not reach the platform: {error}</p>}
          {!error && session.state === "unavailable" && <p className="platform-error" role="alert">Could not reach the platform: {session.message}</p>}
          {!error && !clerk && session.state !== "unavailable" && <LoadingBlock label="Opening owner sign-in" className="platform-form" sinceNavigation>
            <Skeleton width={48} height={12} /><Skeleton height={36} />
            <Skeleton width={64} height={12} /><Skeleton height={36} />
          </LoadingBlock>}
          {!error && clerk && <ClerkSignIn publishableKey={config?.publishableKey} signOut={signOut} onError={setError} onComplete={finish} />}
        </Card>
      </main>
    </div>
  );
}

function ClerkSignIn({ publishableKey, signOut, onError, onComplete }: { publishableKey?: string | null; signOut: boolean; onError: (error: string) => void; onComplete: () => void }) {
  const element = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let active = true;
    let unsubscribe: (() => void) | undefined;
    let exchanging = false;
    const loadScript = (src: string, key?: string) => new Promise<void>((resolve, reject) => {
      const script = document.createElement("script");
      script.src = src; script.async = true; script.crossOrigin = "anonymous";
      if (key) script.dataset.clerkPublishableKey = key;
      script.onload = () => resolve(); script.onerror = () => reject(new Error("Could not load Google sign-in."));
      document.head.appendChild(script);
    });
    const start = async () => {
      if (!publishableKey) throw new Error("Google sign-in is not configured on this installation.");
      const encoded = publishableKey.split("_")[2];
      const decoded = atob(encoded);
      if (!decoded.endsWith("$") || !/^[a-z0-9.-]+\$$/i.test(decoded)) throw new Error("Invalid sign-in configuration.");
      const domain = decoded.slice(0, -1);
      if (!window.__internal_ClerkUICtor) await loadScript(`https://${domain}/npm/@clerk/ui@1/dist/ui.browser.js`);
      if (!window.Clerk) await loadScript(`https://${domain}/npm/@clerk/clerk-js@6/dist/clerk.browser.js`, publishableKey);
      const clerk = window.Clerk;
      if (!clerk) throw new Error("Could not start Google sign-in.");
      await clerk.load({ ui: { ClerkUI: window.__internal_ClerkUICtor } });
      if (!active) return;
      // The owner page signed out of Konae; end the Clerk session too, or the
      // widget would sign the owner straight back in.
      if (signOut && clerk.session) await clerk.signOut();
      const exchange = async () => {
        if (!active || !clerk.session || exchanging) return;
        exchanging = true;
        try {
          const token = await clerk.session.getToken();
          if (!token) throw new Error("Could not obtain a sign-in token.");
          const response = await fetch("/_konae/owner/clerk", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ token }) });
          const body = await response.json().catch(() => ({})) as { error?: string };
          if (!response.ok) throw new Error(body.error ?? "Sign-in failed.");
          if (active) onComplete();
        } catch (cause) { if (active) onError(cause instanceof Error ? cause.message : String(cause)); }
        finally { exchanging = false; }
      };
      unsubscribe = clerk.addListener(() => { void exchange(); });
      if (clerk.session) await exchange(); else if (element.current) clerk.mountSignIn(element.current);
    };
    void start().catch((cause) => { if (active) onError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { active = false; unsubscribe?.(); if (element.current && window.Clerk) window.Clerk.unmountSignIn(element.current); };
  }, [publishableKey, signOut, onError, onComplete]);
  return <div ref={element} />;
}
