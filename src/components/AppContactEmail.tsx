import { Mail } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { readContactEmail, setContactEmail, type ContactEmailState } from "../deployments/client";
import { useLoadingHold } from "./loading-hold";
import { isSignedOutError } from "./owner-session";
import { Badge, Button, Field, Input, LoadingBlock } from "./ui";

/** One sentence for an app whose owner notices have nowhere to go, or null when they do. */
export function missingContactSentence(state: ContactEmailState | null) {
  if (!state || state.contactEmail || state.ownerNotices.length === 0) return null;
  return `${state.ownerNotices.join(", ")} ${state.ownerNotices.length === 1 ? "sends" : "send"} a notice to the owner, but this app has no contact email, so no notice is sent.`;
}

/**
 * The app's contact email, where notices to the owner go. A change shows
 * what the Kernel kept, read back from konae.json.
 */
export function AppContactEmail({ deploymentId, onSignedOut }: { deploymentId: string; onSignedOut: () => void }) {
  const [state, setState] = useState<ContactEmailState | null>(null);
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [reload, setReload] = useState(0);
  const errorRef = useRef<HTMLParagraphElement>(null);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { if (error) errorRef.current?.focus(); }, [error]);
  useEffect(() => {
    let current = true;
    setError("");
    readContactEmail(deploymentId).then((value) => {
      if (!current) return;
      setState(value);
      setEmail(value.contactEmail ?? "");
    }, (cause) => {
      if (!current) return;
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) onSignedOut();
    });
    return () => { current = false; };
  }, [deploymentId, reload, onSignedOut]);

  const save = async (next: string | null) => {
    if (busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await setContactEmail(deploymentId, next);
      const readBack = await readContactEmail(deploymentId);
      if (!alive.current) return;
      setState(readBack);
      setEmail(readBack.contactEmail ?? "");
      setNotice(readBack.contactEmail ? "Saved." : "Contact email removed.");
    } catch (cause) {
      if (!alive.current) return;
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) onSignedOut();
    } finally { if (alive.current) setBusy(false); }
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const trimmed = email.trim();
    if (trimmed && trimmed !== state?.contactEmail) void save(trimmed);
  };

  const missing = missingContactSentence(state);
  const pending = useLoadingHold(!state && !error);
  const kept = "It is kept in konae.json, so an export carries it.";
  return <section className="domain-step" aria-label="Contact email" aria-busy={busy || !state}>
    <header className="domain-heading"><Mail size={20} /><h3>Contact email</h3></header>
    {error && <p className="domain-error" role="alert" tabIndex={-1} ref={errorRef}>{error}</p>}
    {pending && <LoadingBlock label="Reading the contact email" rows={1} />}
    {!state && error && <Button onClick={() => setReload((value) => value + 1)}>Retry loading</Button>}
    {state && !pending && <>
      {missing && <div className="domain-status"><Badge tone="warning">Notices not sent</Badge></div>}
      <p>{missing ?? (state.contactEmail ? `Notices to the owner go to ${state.contactEmail}. ${kept}` : `Where the app sends a notice to its owner. ${kept}`)}</p>
      <form onSubmit={submit} className="domain-form">
        <Field label="Email" help="Publishing keeps this address, unless the folder you publish from names another in konae.json. Without one, Konae uses your owner email when it knows it.">
          <Input type="email" value={email} onChange={(event) => { setEmail(event.target.value); setNotice(""); setError(""); }} autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="email" placeholder="owner@example.org" disabled={busy} />
        </Field>
        <div className="domain-actions">
          <Button type="submit" variant="primary" loading={busy} disabled={!email.trim() || email.trim() === state.contactEmail}>{state.contactEmail ? "Change email" : "Save email"}</Button>
          {state.contactEmail && <Button variant="danger" disabled={busy} onClick={() => void save(null)}>Remove email</Button>}
        </div>
      </form>
      {notice && <p className="domain-notice" role="status">{notice}</p>}
    </>}
  </section>;
}
