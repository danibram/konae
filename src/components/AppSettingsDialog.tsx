import { Check, Copy, Download, ExternalLink, Globe2 } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { clearDomain, domainTrafficTarget, readDomain, setDomain, type DomainView } from "../deployments/domains";
import { hostnameLabel, setHostname } from "../deployments/hostnames";
import { isSharedOriginRefusal, pathAddressLabel, pathNeedsDomain, readPath, setPath, type PathChange, type PathView } from "../deployments/paths";
import { checkHostname } from "../lib/domain-name";
import { checkPathName } from "../lib/path-name";
import { AppContactEmail } from "./AppContactEmail";
import { AppImport } from "./AppImport";
import { AppMaintenance } from "./AppMaintenance";
import { useLoadingHold } from "./loading-hold";
import { isSignedOutError } from "./owner-session";
import { Badge, Button, Dialog, Field, Input, LoadingBlock, Switch, TabBar } from "./ui";
import "./app-settings.css";

type Props = {
  projectName: string;
  /** The app an import replaces, and the visibility its imported sources keep. */
  importTarget?: { project: { id: string; name: string; slug: string }; visibility: "public" | "private"; onImported: () => void };
  deploymentId?: string;
  onClose: () => void;
  onSignedOut: () => void;
  /** The generated address moved; the editor shows the new url everywhere. */
  onAddressMoved?: (url: string) => void;
  signInHref: string;
  /** Downloads the app folder, as More > Export does. */
  onExport?: () => void;
};

type Section = "addresses" | "email" | "move";
const sections: { id: Section; label: string }[] = [
  { id: "addresses", label: "Addresses" },
  { id: "email", label: "Contact email" },
  { id: "move", label: "Move this app" },
];

export function AppSettingsDialog({ projectName, importTarget, deploymentId, onClose, onSignedOut, onAddressMoved, signInHref, onExport }: Props) {
  // Everyday settings apart from moving the app; every part stays mounted, so a switch loses nothing typed.
  const [section, setSection] = useState<Section>("addresses");
  const [view, setView] = useState<DomainView | null>(null);
  const [hostname, setHostname] = useState("");
  const [reading, setLoading] = useState(Boolean(deploymentId));
  // The loader follows the hold. A quick read shows none, and once shown it stays 400 ms.
  const loading = useLoadingHold(reading);
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
    {deploymentId && <TabBar label="App settings" className="app-settings-tabs" value={section} onChange={setSection}
      items={sections.map((item) => ({ ...item, tabId: `app-settings-tab-${item.id}`, controls: `app-settings-${item.id}` }))} />}
    <div className="app-settings-body" aria-busy={busy || loading}>
      {!deploymentId ? <>
        <header className="domain-heading"><Globe2 size={20} /><h3>Addresses</h3></header>
        <p>Deploy this app first. Then its addresses appear here.</p>
      </> : <>
      <div className="app-settings-section" role="tabpanel" id="app-settings-addresses" aria-labelledby="app-settings-tab-addresses" hidden={section !== "addresses"}>
        {loading && <LoadingBlock label="Reading domain settings" rows={2} />}
        {error && <p className="domain-error" role="alert" tabIndex={-1} ref={errorRef}>{error}</p>}
        {error && view && reloadOffered && !signedOut && <Button disabled={busy} onClick={() => { setConfirmation(null); setReload((value) => value + 1); }}>Reload domain settings</Button>}
        {signedOut && <a href={signInHref}>Sign in again to manage this app</a>}
        {!loading && !view && !signedOut && <Button onClick={() => setReload((value) => value + 1)}>Retry loading</Button>}
        {!loading && view && <>
          <section className="domain-step" aria-label="Generated address">
            <h4>Generated address</h4>
            <a className="domain-address" href={view.generatedUrl} target="_blank" rel="noreferrer">{view.generatedUrl}<ExternalLink size={14} /></a>
            <p>Always on. Sessions on this address stay on this app's own origin.</p>
            <GeneratedAddressName key={deploymentId} deploymentId={deploymentId} url={view.generatedUrl} disabled={busy || signedOut} onSignedOut={() => { setSignedOut(true); onSignedOut(); }} onMoved={(url) => { setView((current) => current && { ...current, generatedUrl: url }); onAddressMoved?.(url); }} />
          </section>
          <section className="domain-step" aria-label="Custom domain">
            <h4>Custom domain</h4>
            <div className="domain-status"><Badge tone={view.status === "active" ? "success" : view.status === "pending" ? "warning" : "neutral"}>{view.status === "active" ? "Ownership verified" : view.status === "pending" ? "Waiting for TXT" : "No custom domain"}</Badge></div>
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
                <p>Add this TXT record at your DNS provider. DNS updates can take a while to arrive.</p>
                <dl className="domain-record">
                  <div><dt>Type</dt><dd>TXT</dd></div>
                  <CopyRecord key={view.record.name} label="Name" value={view.record.name} />
                  <CopyRecord key={view.record.value} label="Value" value={view.record.value} />
                </dl>
                <p>Some providers append your domain to the name. Enter the part before your zone, so the final name matches the full name above.</p>
                {view.dns && <p className="domain-dns-result" role="status">{view.dns}</p>}
                {view.status === "active" && <p>This hostname is verified. It stays verified until you remove it; refreshing does not check the TXT again.</p>}
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
          </section>
        </>}
        <PathAddress key={deploymentId} deploymentId={deploymentId} signInHref={signInHref} onSignedOut={onSignedOut} />
      </div>
      <div className="app-settings-section" role="tabpanel" id="app-settings-email" aria-labelledby="app-settings-tab-email" hidden={section !== "email"}>
        <AppContactEmail key={`contact-${deploymentId}`} deploymentId={deploymentId} onSignedOut={onSignedOut} />
      </div>
      <div className="app-settings-section" role="tabpanel" id="app-settings-move" aria-labelledby="app-settings-tab-move" hidden={section !== "move"}>
        <p>To take this app to another machine with its accounts, rows and rules, export its folder here and import it there. Pause it first so nothing changes in between.</p>
        {onExport && <section className="domain-step" aria-label="Export">
          <header className="domain-heading"><Download size={20} /><h3>Export</h3></header>
          <p>Downloads the app folder as a .tar with its files, accounts, rows, uploads and rules.</p>
          <Button onClick={onExport}><Download size={14} aria-hidden="true" /> Export app folder</Button>
        </section>}
        <AppMaintenance key={`maintenance-${deploymentId}`} deploymentId={deploymentId} onSignedOut={onSignedOut} />
        {importTarget && <AppImport deploymentId={deploymentId} {...importTarget} onSignedOut={onSignedOut} />}
      </div>
      </>}
    </div>
  </Dialog>;
}

/** Moves the generated address to another name. The Kernel checks the name and its refusal is shown as it comes; the change counts once domain_read shows the new address. */
function GeneratedAddressName({ deploymentId, url, disabled, onMoved, onSignedOut }: { deploymentId: string; url: string; disabled: boolean; onMoved: (url: string) => void; onSignedOut: () => void }) {
  const current = hostnameLabel(url);
  const [name, setName] = useState(current);
  useEffect(() => { setName(current); }, [current]);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const confirmationRef = useRef<HTMLElement>(null);
  const errorRef = useRef<HTMLParagraphElement>(null);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { if (confirming) confirmationRef.current?.focus(); }, [confirming]);
  useEffect(() => { if (error) errorRef.current?.focus(); }, [error]);
  const wanted = name.trim().toLowerCase();
  const ready = Boolean(wanted) && wanted !== current;

  const move = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const moved = await setHostname(deploymentId, wanted);
      const readBack = await readDomain(deploymentId);
      if (!alive.current) return;
      if (readBack.generatedUrl !== moved.url) throw new Error("The address does not show the change yet. Reload the app settings and check it again.");
      setConfirming(false);
      setNotice(`This app now answers at ${moved.url}. ${url} redirects there. If the app offers Google sign-in, add ${moved.googleCallback} to its Google client's redirect URIs.`);
      onMoved(moved.url);
    } catch (cause) {
      if (!alive.current) return;
      setConfirming(false);
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) onSignedOut();
    } finally { if (alive.current) setBusy(false); }
  };

  return <>
    <form className="domain-form" onSubmit={(event) => { event.preventDefault(); if (ready) { setError(""); setNotice(""); setConfirming(true); } }}>
      <Field label="Name" help="Letters, digits and hyphens, such as notes. Another app's name, now or before, is refused.">
        <Input value={name} onChange={(event) => { setName(event.target.value); setConfirming(false); setError(""); }} autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" placeholder="notes" disabled={disabled || busy} />
      </Field>
      <Button type="submit" disabled={disabled || busy || confirming || !ready}>Change address</Button>
    </form>
    {error && <p className="domain-error" role="alert" tabIndex={-1} ref={errorRef}>{error}</p>}
    {notice && <p className="domain-notice" role="status">{notice}</p>}
    {confirming && <section className="domain-confirm" aria-label="Confirm address change" tabIndex={-1} ref={confirmationRef}>
      <h4>Move this app to {wanted}?</h4>
      <p>{`${url} will redirect to the new address, and no other app can take it. People signed in to the app sign in again there. If the app offers Google sign-in, add the new address's callback to its Google client. A service that calls the app's HTTP routes needs the new address.`}</p>
      <div className="domain-actions">
        <Button onClick={() => setConfirming(false)} disabled={busy}>Cancel</Button>
        <Button variant="primary" loading={busy} onClick={() => void move()}>Move address</Button>
      </div>
    </section>}
  </>;
}

type PathConfirmation = { kind: "accept"; change: PathChange; message: string } | { kind: "remove-name" };

/** The path address of the app: its switch, its addresses and its name. Each change reads the path back before it reports success. */
function PathAddress({ deploymentId, signInHref, onSignedOut }: { deploymentId: string; signInHref: string; onSignedOut: () => void }) {
  const [view, setView] = useState<PathView | null>(null);
  const [reading, setLoading] = useState(true);
  const loading = useLoadingHold(reading);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [signedOut, setSignedOut] = useState(false);
  const [reload, setReload] = useState(0);
  const [name, setName] = useState("");
  const [confirmation, setConfirmation] = useState<PathConfirmation | null>(null);
  const confirmationRef = useRef<HTMLElement>(null);
  const errorRef = useRef<HTMLParagraphElement>(null);
  const noticeRef = useRef<HTMLParagraphElement>(null);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { if (confirmation) confirmationRef.current?.focus(); }, [confirmation]);
  useEffect(() => { if (error) errorRef.current?.focus(); }, [error]);
  useEffect(() => { if (notice) noticeRef.current?.focus(); }, [notice]);
  useEffect(() => {
    let current = true;
    setLoading(true);
    setError("");
    readPath(deploymentId).then((value) => {
      if (!current) return;
      setView(value);
      setName(value.name ?? "");
    }, (cause) => {
      if (!current) return;
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) { setSignedOut(true); onSignedOut(); }
    }).finally(() => { if (current) setLoading(false); });
    return () => { current = false; };
  }, [deploymentId, reload, onSignedOut]);

  // A change counts only when path_read shows it. A shared-origin refusal asks the owner, and the Kernel's words are the ones shown.
  const apply = async (next: PathChange, operation: () => Promise<unknown> = () => setPath(deploymentId, next)) => {
    if (!view || busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await operation();
      const readBack = await readPath(deploymentId);
      if (!alive.current) return;
      if (readBack.enabled !== next.enabled || (next.name !== undefined && readBack.name !== next.name)) {
        throw new Error("The path address does not show the change yet. Reload the app settings and check it again.");
      }
      setView(readBack);
      setName(readBack.name ?? "");
      setConfirmation(null);
      setNotice(next.name === null ? "Name removed. The id address keeps answering." : next.name !== undefined ? "Name saved." : readBack.enabled ? "Path address on." : "Path address off. Its name is free for another app.");
    } catch (cause) {
      if (!alive.current) return;
      if (isSharedOriginRefusal(cause)) {
        setConfirmation({ kind: "accept", change: next, message: cause instanceof Error ? cause.message : String(cause) });
      } else {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
      if (isSignedOutError(cause)) { setSignedOut(true); onSignedOut(); }
    } finally { if (alive.current) setBusy(false); }
  };

  // The Kernel decides whether this app has accounts; its path.shared-origin refusal opens the confirmation with its own words.
  const turnOn = (next: PathChange) => { void apply(next); };

  const checked = name.trim() ? checkPathName(name) : null;
  const nameError = checked && !checked.ok ? checked.message : "";
  const nameReady = Boolean(checked?.ok && checked.name !== (view?.name ?? ""));

  const saveName = (event: FormEvent) => {
    event.preventDefault();
    if (!checked?.ok || !nameReady) return;
    turnOn({ enabled: true, name: checked.name });
  };

  return <section className="domain-step" aria-label="Path address" aria-busy={busy || loading}>
    <h4>Path address</h4>
    {loading && <LoadingBlock label="Reading path address" rows={2} />}
    {error && <p className="domain-error" role="alert" tabIndex={-1} ref={errorRef}>{error}</p>}
    {signedOut && <a href={signInHref}>Sign in again to manage this app</a>}
    {!loading && !view && !signedOut && <Button onClick={() => setReload((value) => value + 1)}>Retry loading</Button>}
    {!loading && view && pathNeedsDomain(view) && <p className="domain-notice" role="status">This installation has no short links. Its operator turns them on.</p>}
    {!loading && view && !pathNeedsDomain(view) && <>
      <p>Answers on the installation's domain, under /p/. Other apps on that domain share its origin.</p>
      <Switch checked={view.enabled} label="Answer on the path" disabled={busy || signedOut || Boolean(confirmation)} onChange={(next) => {
        if (next) turnOn({ enabled: true });
        else void apply({ enabled: false });
      }} />
      {view.enabled && <>
        <dl className="domain-record">
          {view.addresses.map((url, index) => <div key={url}>
            <dt>{pathAddressLabel(index)}</dt>
            <dd><a className="domain-address" href={url} target="_blank" rel="noreferrer">{url}<ExternalLink size={14} /></a><CopyButton label={`Copy ${url}`} value={url} /></dd>
          </div>)}
        </dl>
        <form onSubmit={saveName} className="domain-form">
          <Field label="Name" help={nameError || "3 to 40 letters, digits and hyphens, such as spring-menu. The id address keeps answering."}>
            <Input value={name} aria-invalid={Boolean(nameError) || undefined} onChange={(event) => { setName(event.target.value); setConfirmation(null); setError(""); setNotice(""); }} autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" placeholder="spring-menu" disabled={busy || signedOut} />
          </Field>
          <Button type="submit" variant="primary" loading={busy} disabled={signedOut || Boolean(confirmation) || !nameReady}>{view.name ? "Change name" : "Save name"}</Button>
        </form>
        {view.name && <Button variant="danger" disabled={busy || signedOut || Boolean(confirmation)} onClick={() => { setError(""); setConfirmation({ kind: "remove-name" }); }}>Remove name</Button>}
      </>}
    </>}
    {notice && <p className="domain-notice" role="status" tabIndex={-1} ref={noticeRef}>{notice}</p>}
    {confirmation && <section className="domain-confirm" aria-label="Confirm path address" tabIndex={-1} ref={confirmationRef}>
      {confirmation.kind === "accept" ? <>
        <h4>Share this origin?</h4>
        <p>{confirmation.message}</p>
        <div className="domain-actions">
          <Button onClick={() => setConfirmation(null)} disabled={busy}>Cancel</Button>
          <Button variant="primary" loading={busy} onClick={() => void apply({ ...confirmation.change, acceptSharedOrigin: true })}>Accept and turn on</Button>
        </div>
      </> : <>
        <h4>Remove this name?</h4>
        <p>{`/p/${view?.name ?? "name"} stops answering and becomes free for another app. The id address keeps answering.`}</p>
        <div className="domain-actions">
          <Button onClick={() => setConfirmation(null)} disabled={busy}>Cancel</Button>
          <Button variant="danger" loading={busy} onClick={() => void apply({ enabled: true, name: null })}>Remove name</Button>
        </div>
      </>}
    </section>}
  </section>;
}

function CopyButton({ label, value }: { label: string; value: string }) {
  const [feedback, setFeedback] = useState("");
  return <>
    <Button aria-label={label} onClick={async () => {
      try { await navigator.clipboard.writeText(value); setFeedback("Copied"); } catch { setFeedback("Could not copy. Select and copy the value above."); }
    }}>{feedback === "Copied" ? <Check size={14} /> : <Copy size={14} />}Copy</Button>
    {feedback && <small role="status">{feedback}</small>}
  </>;
}

function CopyRecord({ label, value }: { label: string; value: string }) {
  return <div><dt>{label}</dt><dd><code>{value}</code><CopyButton label={`Copy TXT ${label.toLowerCase()}`} value={value} /></dd></div>;
}

function TrafficInstructions({ view }: { view: DomainView }) {
  const target = domainTrafficTarget(view.generatedUrl);
  return <section className="domain-step" aria-label="Traffic and HTTPS setup">
    <h4>2. Connect traffic and HTTPS</h4>
    <Badge tone="neutral">Not checked</Badge>
    {target.local ? <p>This installation runs locally. Public DNS cannot point to localhost. Your hosting provider must supply a public DNS target and an HTTPS certificate.</p> : <>
      <p>For a subdomain, point a CNAME record at <code>{target.hostname}</code>. For a root domain, ask your hosting provider for the required ALIAS or IP records.</p>
      <p>Your hosting provider must configure HTTPS for <code>{view.hostname}</code> and forward requests to this installation.</p>
    </>}
    <p>A verified TXT proves ownership. It does not check traffic routing or the HTTPS certificate.</p>
  </section>;
}
