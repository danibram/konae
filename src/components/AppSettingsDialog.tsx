import { Check, Copy, ExternalLink, Globe2 } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { clearDomain, domainTrafficTarget, readDomain, setDomain, type DomainView } from "../deployments/domains";
import { checkHostname } from "../lib/domain-name";
import { isSignedOutError } from "./owner-session";
import { Badge, Button, Dialog, Field, Input, LoadingBlock } from "./ui";
import "./app-settings.css";

type Props = {
  projectName: string;
  deploymentId?: string;
  onClose: () => void;
  onDeploy: () => void;
  onSignedOut: () => void;
  signInHref: string;
};

export function AppSettingsDialog({ projectName, deploymentId, onClose, onDeploy, onSignedOut, signInHref }: Props) {
  const [view, setView] = useState<DomainView | null>(null);
  const [hostname, setHostname] = useState("");
  const [loading, setLoading] = useState(Boolean(deploymentId));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [inputError, setInputError] = useState("");
  const [reloadOffered, setReloadOffered] = useState(false);
  const [notice, setNotice] = useState("");
  const [signedOut, setSignedOut] = useState(false);
  const [reload, setReload] = useState(0);
  const [confirmation, setConfirmation] = useState<{ kind: "replace" | "remove"; hostname: string } | null>(null);
  const confirmationRef = useRef<HTMLElement>(null);
  const errorRef = useRef<HTMLParagraphElement>(null);
  const noticeRef = useRef<HTMLParagraphElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { if (confirmation) confirmationRef.current?.focus(); }, [confirmation]);
  useEffect(() => { if (error) errorRef.current?.focus(); }, [error]);
  useEffect(() => { if (notice) noticeRef.current?.focus(); }, [notice]);
  useEffect(() => { if (inputError) inputRef.current?.focus(); }, [inputError]);
  useEffect(() => {
    if (!deploymentId) return;
    let current = true;
    setLoading(true);
    setError("");
    readDomain(deploymentId).then((value) => {
      if (!current) return;
      setView(value);
      setHostname(value.hostname ?? "");
    }, (cause) => {
      if (!current) return;
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) { setSignedOut(true); onSignedOut(); }
    }).finally(() => { if (current) setLoading(false); });
    return () => { current = false; };
  }, [deploymentId, reload, onSignedOut]);

  const perform = async (operation: "set" | "remove", nextHostname: string) => {
    if (!deploymentId || busy) return;
    setBusy(true);
    setError("");
    setReloadOffered(false);
    setNotice("");
    try {
      const current = await readDomain(deploymentId);
      if (current.hostname !== view?.hostname) {
        setReloadOffered(true);
        throw new Error("The domain changed elsewhere. Reload its settings before changing it.");
      }
      const result = operation === "remove" ? await clearDomain(deploymentId) : await setDomain(deploymentId, nextHostname);
      if (!alive.current) return;
      if (operation === "remove") {
        setView({ deploymentId, hostname: null, status: "none", generatedUrl: result.generatedUrl, rules: view?.rules });
        setHostname("");
        setNotice("Domain removed. The generated address still works.");
      } else {
        const domain = result as DomainView;
        setView({ ...domain, rules: view?.rules });
        setHostname(domain.hostname ?? "");
        setNotice(domain.status === "active" ? "Ownership verified. Your hosting provider must also connect traffic and HTTPS." : "Ownership is still pending. Check the TXT record below and try again.");
      }
      setConfirmation(null);
    } catch (cause) {
      if (!alive.current) return;
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) { setSignedOut(true); onSignedOut(); }
    } finally { if (alive.current) setBusy(false); }
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!view || busy) return;
    const checked = checkHostname(hostname, view.rules ?? { suffix: "", installationHosts: [] });
    if (!checked.ok) {
      setInputError(/:\/\/|[/?#]/.test(hostname) ? "Enter only the hostname, without https:// or a path." : checked.message);
      return;
    }
    setInputError("");
    if (view.hostname && view.hostname !== checked.hostname) {
      setError("");
      setConfirmation({ kind: "replace", hostname: checked.hostname });
    } else void perform("set", checked.hostname);
  };

  return <Dialog title="App settings" description={projectName} onClose={() => { if (!busy) onClose(); }} className="app-settings-dialog">
    <div className="app-settings-body" aria-busy={busy || loading}>
      <header className="domain-heading"><Globe2 size={20} /><h3>Custom domain</h3></header>
      {!deploymentId ? <>
        <p>Publish this app first. Then connect a domain you own.</p>
        <Button variant="primary" onClick={onDeploy}>Deploy app</Button>
      </> : <>
        {loading && <LoadingBlock label="Reading domain settings" rows={2} />}
        {error && <p className="domain-error" role="alert" tabIndex={-1} ref={errorRef}>{error}</p>}
        {error && view && reloadOffered && !signedOut && <Button disabled={busy} onClick={() => { setConfirmation(null); setReload((value) => value + 1); }}>Reload domain settings</Button>}
        {signedOut && <a href={signInHref}>Sign in again to manage this app</a>}
        {!loading && !view && !signedOut && <Button onClick={() => setReload((value) => value + 1)}>Retry loading</Button>}
        {!loading && view && <>
          <div className="domain-status"><Badge tone={view.status === "active" ? "success" : view.status === "pending" ? "warning" : "neutral"}>{view.status === "active" ? "Ownership verified" : view.status === "pending" ? "Waiting for TXT" : "No custom domain"}</Badge></div>
          <p>Your app keeps its generated address.</p>
          <a className="domain-address" href={view.generatedUrl} target="_blank" rel="noreferrer">{view.generatedUrl}<ExternalLink size={14} /></a>
          <form onSubmit={submit} className="domain-form">
            <Field label="Domain" help={inputError || "Use a hostname such as notes.example.org, without https:// or a path."}>
              <Input ref={inputRef} value={hostname} aria-invalid={Boolean(inputError) || undefined} onChange={(event) => { setHostname(event.target.value); setConfirmation(null); setError(""); setInputError(""); }} autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" placeholder="notes.example.org" disabled={busy || signedOut} />
            </Field>
            <Button type="submit" variant="primary" loading={busy} disabled={signedOut || Boolean(confirmation)}>{view.hostname === hostname.trim().toLowerCase() ? view.status === "active" ? "Refresh domain status" : "Check TXT record" : view.hostname ? "Change domain" : "Add domain"}</Button>
          </form>
          {notice && <p className="domain-notice" role="status" tabIndex={-1} ref={noticeRef}>{notice}</p>}
          {view.hostname && view.record && <>
            <section className="domain-step" aria-label="DNS ownership record">
              <h4>1. Prove you own {view.hostname}</h4>
              <p>Add this TXT record at your DNS provider. DNS updates can take time to reach the Kernel.</p>
              <dl className="domain-record">
                <div><dt>Type</dt><dd>TXT</dd></div>
                <CopyRecord key={view.record.name} label="Name" value={view.record.name} />
                <CopyRecord key={view.record.value} label="Value" value={view.record.value} />
              </dl>
              <p>Some providers append your domain to the name. Enter the part before your zone, so the final name matches the full name above.</p>
              {view.dns && <p className="domain-dns-result" role="status">{view.dns}</p>}
              {view.status === "active" && <p>The Kernel accepts this hostname. Ownership stays verified until you remove it; refreshing does not recheck the TXT.</p>}
            </section>
            <TrafficInstructions view={view} />
            <p className="domain-portability">Moving this app to another machine issues a new TXT token. Verify it at the destination before switching traffic.</p>
            {view.status === "active" && view.url && <a className="domain-address" href={view.url} target="_blank" rel="noreferrer">Open domain<ExternalLink size={14} /></a>}
            <Button variant="danger" disabled={busy || signedOut} onClick={() => { setError(""); setConfirmation({ kind: "remove", hostname: view.hostname! }); }}>Remove domain</Button>
          </>}
          {confirmation && <section className="domain-confirm" aria-label="Confirm domain change" tabIndex={-1} ref={confirmationRef}>
            <h4>{confirmation.kind === "remove" ? "Remove this domain?" : "Replace the current domain?"}</h4>
            <p>{confirmation.kind === "remove" ? `${confirmation.hostname} will stop serving this app.` : `${view.hostname} will stop serving this app when you switch to ${confirmation.hostname}, even if the new TXT is still pending.`} The generated address keeps working.</p>
            <div className="domain-actions">
              <Button onClick={() => setConfirmation(null)} disabled={busy}>Cancel change</Button>
              <Button variant="danger" loading={busy} onClick={() => void perform(confirmation.kind === "remove" ? "remove" : "set", confirmation.hostname)}>{confirmation.kind === "remove" ? "Confirm removal" : "Replace domain"}</Button>
            </div>
          </section>}
          <p className="domain-agent-note">You can also ask the agent to manage this app's domain. It uses these same Kernel checks and asks before changing it.</p>
        </>}
      </>}
    </div>
  </Dialog>;
}

function CopyRecord({ label, value }: { label: string; value: string }) {
  const [feedback, setFeedback] = useState("");
  return <div><dt>{label}</dt><dd><code>{value}</code><Button aria-label={`Copy TXT ${label.toLowerCase()}`} onClick={async () => {
    try { await navigator.clipboard.writeText(value); setFeedback("Copied"); } catch { setFeedback("Could not copy. Select and copy the value above."); }
  }}>{feedback === "Copied" ? <Check size={14} /> : <Copy size={14} />}Copy</Button></dd>{feedback && <small role="status">{feedback}</small>}</div>;
}

function TrafficInstructions({ view }: { view: DomainView }) {
  const target = domainTrafficTarget(view.generatedUrl);
  return <section className="domain-step" aria-label="Traffic and HTTPS setup">
    <h4>2. Connect traffic and HTTPS</h4>
    <Badge tone="neutral">Not checked</Badge>
    {target.local ? <p>This installation runs locally. Public DNS cannot point to localhost. Your hosting provider must supply a public DNS target and an HTTPS certificate.</p> : <>
      <p>For a subdomain, point a CNAME record at <code>{target.hostname}</code>. For a root domain, ask your hosting provider for the required ALIAS or IP records.</p>
      <p>Your hosting provider must configure HTTPS for <code>{view.hostname}</code> and forward its hostname to the Kernel.</p>
    </>}
    <p>A verified TXT proves ownership. It does not check traffic routing or the HTTPS certificate.</p>
  </section>;
}
