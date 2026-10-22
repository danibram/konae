import { ArrowLeft, Box, KeyRound, LogOut } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { platformMcp } from "../mcp/client";
import { rememberOwnerEmail, useOwnerSession, type OwnerSession } from "./owner-session";
import { useLoadingHold } from "./loading-hold";
import { safeReturnPath } from "./return-path";
import { Badge, Button, Card, Field, Input, LoadingBlock, Select, Skeleton, Textarea } from "./ui";

type Credential = { tokenId: string; scopes: string; expiresAt: number };
type ClerkBrowser = { load: (options: object) => Promise<void>; session: { getToken: () => Promise<string | null> } | null; mountSignIn: (element: HTMLElement) => void; unmountSignIn: (element: HTMLElement) => void; addListener: (listener: () => void) => () => void; signOut: () => Promise<void> };
declare global { interface Window { Clerk?: ClerkBrowser; __internal_ClerkUICtor?: unknown } }

/**
 * Owner sign-in for this installation, inside the editor so it looks like the
 * rest of the workspace and can send the owner back to the project they came
 * from. Agent tokens are managed here too.
 */
export function PlatformAccess() {
  const returnTo = useMemo(() => safeReturnPath(new URLSearchParams(window.location.search).get("returnTo")), []);
  const [liveSession, refresh] = useOwnerSession();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [setupKey, setSetupKey] = useState("local-development-key");
  const [code, setCode] = useState("");
  const [pendingEmail, setPendingEmail] = useState<string | null>(null);
  const [creatingAccount, setCreatingAccount] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [config, setConfig] = useState<{ mode: "local" | "clerk"; publishableKey?: string | null; admission?: "setup-key" | "code" } | null>(null);
  // The setup key creates only an installation's first owner. Every other
  // account needs an invitation code, so the form asks for one unless the
  // Kernel says the key still applies.
  const usesSetupKey = config?.admission === "setup-key";
  // A check that took long enough to show its skeleton keeps it for a moment, so the card never flashes.
  const waiting = useLoadingHold(liveSession.state === "checking" || (liveSession.state === "signed-out" && !config && !error), { sinceNavigation: true });
  const session: OwnerSession = waiting && liveSession.state !== "checking" ? { state: "checking" } : liveSession;
  useEffect(() => { void fetch("/_littlebox/owner/config").then(async (response) => {
    if (!response.ok) throw new Error("Could not load sign-in settings.");
    setConfig(await response.json());
  }).catch((cause) => setError(cause instanceof Error ? cause.message : String(cause))); }, []);

  const backHref = returnTo && !returnTo.startsWith("/oauth/") ? returnTo : "/";
  const backLabel = backHref.startsWith("/app/") ? "Back to project" : "Back to projects";

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const action = ((event.nativeEvent as SubmitEvent).submitter as HTMLButtonElement | null)?.value === "signup" ? "signup" : "login";
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const response = await fetch(`/_littlebox/owner/${action}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(action === "signup"
          ? { email, password, ...(usesSetupKey ? { setupKey } : { code }), ...(returnTo?.startsWith("/oauth/authorize?") ? { returnTo } : {}) }
          : { email, password }),
      });
      const body = await response.json().catch(() => ({})) as { error?: string; user?: { name?: string }; result?: { message?: string } };
      if (!response.ok) throw new Error(body.error || (action === "signup" ? "Could not create the owner account." : "Sign in failed."));
      // An installation that verifies email answers a signup with 202 and a
      // link sent to the address; the account is not active yet.
      if (response.status === 202) {
        setPendingEmail(email.trim());
        setPassword("");
        setCode("");
        return;
      }
      rememberOwnerEmail(body.user?.name ?? email);
      setPassword("");
      setSetupKey("");
      setCode("");
      if (returnTo) {
        window.location.assign(returnTo);
        return;
      }
      refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  // A pending account asks for its link again through recovery, which sends
  // a new verification link and invalidates the earlier one.
  const resend = async () => {
    if (!pendingEmail) return;
    setBusy(true);
    setError("");
    try {
      const response = await fetch("/_littlebox/owner/recover", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email: pendingEmail }) });
      const body = await response.json().catch(() => ({})) as { error?: string; result?: { message?: string } };
      if (!response.ok) throw new Error(body.error || "Could not send the link.");
      setNotice("We sent a new link. Only the newest link works.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const signOut = async () => {
    await fetch("/_littlebox/owner/logout", { method: "POST" }).catch(() => undefined);
    if (config?.mode === "clerk") await window.Clerk?.signOut().catch(() => undefined);
    rememberOwnerEmail(null);
    refresh();
  };

  return (
    <div className="platform-page">
      <header className="platform-header">
        <a className="platform-brand" href="/"><span><Box size={16} /></span>littlebox</a>
        <a className="ui-button ui-button-sm platform-back" href={backHref}><ArrowLeft size={14} /> {backLabel}</a>
      </header>
      <main className="platform-main">
        <Card className="platform-card">
          <h1>{pendingEmail ? "Check your email" : creatingAccount ? "Create the owner account" : "Owner sign-in"}</h1>
          <p className="platform-lead">Owners deploy apps and manage their data. People who use your apps sign in inside each app instead.</p>

          {(session.state === "checking" || (session.state === "signed-out" && !config && !error)) && <LoadingBlock label="Checking your session" className="platform-form" sinceNavigation>
            <Skeleton width={48} height={12} /><Skeleton height={36} />
            <Skeleton width={64} height={12} /><Skeleton height={36} />
            <div className="platform-actions"><Skeleton width={96} height={42} /><Skeleton width={160} height={42} /></div>
          </LoadingBlock>}
          {session.state === "unavailable" && <p className="platform-error" role="alert">Could not reach the platform: {session.message}</p>}

          {session.state === "signed-out" && config?.mode === "clerk" && <ClerkSignIn publishableKey={config.publishableKey} returnTo={returnTo} onError={setError} onComplete={refresh} />}
          {session.state === "signed-out" && config?.mode === "local" && pendingEmail && (
            <div className="platform-form">
              <p className="platform-note">We sent a link to <strong>{pendingEmail}</strong>. Open it to confirm the address and choose your password. The link works for 24 hours.</p>
              {error && <p className="platform-error" role="alert">{error}</p>}
              {notice && <p role="status">{notice}</p>}
              <div className="platform-actions">
                <Button onClick={() => void resend()} loading={busy}>Send the link again</Button>
                <Button onClick={() => { setPendingEmail(null); setNotice(""); setError(""); }} disabled={busy}>Use another email</Button>
              </div>
            </div>
          )}
          {session.state === "signed-out" && config?.mode === "local" && !pendingEmail && (
            <form className="platform-form" noValidate onSubmit={(event) => {
              event.preventDefault();
              if (!email.trim()) { setError("Add an email."); return; }
              if (password.length < 8) { setError("Use at least 8 characters."); return; }
              void submit(event);
            }}>
              <Field label="Email" help={error === "Add an email." ? error : undefined}><Input type="email" autoComplete="email" aria-invalid={error === "Add an email." || undefined} value={email} onChange={(event) => setEmail(event.target.value)} /></Field>
              <Field label="Password" help={error === "Use at least 8 characters." ? error : undefined}><Input type="password" autoComplete="current-password" aria-invalid={error === "Use at least 8 characters." || undefined} value={password} onChange={(event) => setPassword(event.target.value)} /></Field>
              {creatingAccount && usesSetupKey && (
                <Field label="This computer's setup key" help="Creates the first owner. Filled in for this computer; a hosted install prints its own key where Littlebox starts.">
                  <Input type="password" autoComplete="off" required value={setupKey} onChange={(event) => setSetupKey(event.target.value)} />
                </Field>
              )}
              {creatingAccount && !usesSetupKey && (
                <Field label="Invitation code" help="The operator of this installation gives you one. It works once.">
                  <Input autoComplete="off" spellCheck={false} required value={code} onChange={(event) => setCode(event.target.value)} />
                </Field>
              )}
              {error && error !== "Add an email." && error !== "Use at least 8 characters." && <p className="platform-error" role="alert">{error}</p>}
              {notice && <p role="status">{notice}</p>}
              <div className="platform-actions">
                {creatingAccount ? (
                  <>
                    <Button type="submit" variant="primary" value="signup" loading={busy}>Create owner account</Button>
                    <Button type="button" onClick={() => setCreatingAccount(false)} disabled={busy}>Back to sign in</Button>
                  </>
                ) : (
                  <>
                    <Button type="submit" variant="primary" value="login" loading={busy}>Sign in</Button>
                    <Button type="button" onClick={() => setCreatingAccount(true)} disabled={busy}>Create owner account</Button>
                  </>
                )}
              </div>
            </form>
          )}
          {session.state === "signed-out" && error && config?.mode !== "local" && <p className="platform-error" role="alert">{error}</p>}

          {session.state === "signed-in" && (
            <div className="platform-signed-in">
              <div className="platform-account">
                <span className="platform-avatar" aria-hidden="true"><KeyRound size={15} /></span>
                <div><strong>{session.email ?? "Signed in"}</strong><small>Owner session active in this browser</small></div>
                <Badge tone="success">Signed in</Badge>
              </div>
              <div className="platform-actions">
                <a className="ui-button ui-button-primary" href={backHref}><ArrowLeft size={14} /> {backLabel}</a>
                <Button onClick={() => void signOut()}><LogOut size={14} /> Sign out</Button>
              </div>
              <AgentTokens />
            </div>
          )}
        </Card>
      </main>
    </div>
  );
}

function ClerkSignIn({ publishableKey, returnTo, onError, onComplete }: { publishableKey?: string | null; returnTo: string | null; onError: (error: string) => void; onComplete: () => void }) {
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
      const exchange = async () => {
        if (!active || !clerk.session || exchanging) return;
        exchanging = true;
        try {
          const token = await clerk.session.getToken();
          if (!token) throw new Error("Could not obtain a sign-in token.");
          const response = await fetch("/_littlebox/owner/clerk", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ token }) });
          const body = await response.json().catch(() => ({})) as { error?: string };
          if (!response.ok) throw new Error(body.error ?? "Sign-in failed.");
          if (!active) return;
          if (returnTo) window.location.assign(returnTo); else onComplete();
        } catch (cause) { if (active) onError(cause instanceof Error ? cause.message : String(cause)); }
        finally { exchanging = false; }
      };
      unsubscribe = clerk.addListener(() => { void exchange(); });
      if (clerk.session) await exchange(); else if (element.current) clerk.mountSignIn(element.current);
    };
    void start().catch((cause) => { if (active) onError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { active = false; unsubscribe?.(); if (element.current && window.Clerk) window.Clerk.unmountSignIn(element.current); };
  }, [publishableKey, returnTo, onError, onComplete]);
  return <div ref={element} />;
}

function AgentTokens() {
  const [credentials, setCredentials] = useState<Credential[]>([]);
  const [projects, setProjects] = useState("");
  const [scopes, setScopes] = useState("read");
  const [created, setCreated] = useState<{ token: string; tokenId: string } | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      const result = await platformMcp.call<{ credentials: Credential[] }>("tokens_list");
      setCredentials(result.credentials);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  useEffect(() => { void load(); }, []);

  const create = async () => {
    setBusy(true);
    setError("");
    try {
      const result = await platformMcp.call<{ token: string; tokenId: string }>("token_create", {
        scopes: scopes.split(","),
        projectIds: projects.split(",").map((id) => id.trim()).filter(Boolean),
        ttlSeconds: 86_400,
      });
      setCreated(result);
      await load();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (tokenId: string) => {
    setError("");
    try {
      await platformMcp.call("token_revoke", { tokenId });
      if (created?.tokenId === tokenId) setCreated(null);
      await load();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  return (
    <section className="platform-tokens" aria-label="Agent access">
      <h2>Agent access</h2>
      <p className="platform-note">Give an MCP client a 24-hour token. Configure it with this installation's <code>/mcp</code> URL and an <code>Authorization: Bearer</code> header. Keep tokens out of app source.</p>
      <div className="platform-token-form">
        <Field label="Project IDs" help="Comma-separated. Leave empty for all your projects.">
          <Input value={projects} onChange={(event) => setProjects(event.target.value)} />
        </Field>
        <Field label="Permissions">
          <Select value={scopes} onChange={(event) => setScopes(event.target.value)}>
            <option value="read">Read only</option>
            <option value="read,write,publish">Read, edit and publish</option>
          </Select>
        </Field>
        <Button onClick={() => void create()} loading={busy}>Create 24-hour token</Button>
      </div>
      {created && (
        <Field label="New token, shown once">
          <Textarea readOnly rows={3} value={created.token} onFocus={(event) => event.currentTarget.select()} />
        </Field>
      )}
      {error && <p className="platform-error" role="alert">{error}</p>}
      <h3>Active credentials</h3>
      {credentials.length === 0 ? <p className="platform-note">No agent tokens yet.</p> : (
        <ul className="platform-credentials">
          {credentials.map((credential) => (
            <li key={credential.tokenId}>
              <span>{readScopes(credential.scopes)} · expires {new Date(credential.expiresAt).toLocaleString()}</span>
              <Button size="sm" variant="danger" onClick={() => void revoke(credential.tokenId)}>Revoke</Button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function readScopes(raw: string): string {
  try {
    const parsed = JSON.parse(raw) as unknown;
    return Array.isArray(parsed) ? parsed.join(", ") : String(raw);
  } catch {
    return raw;
  }
}
