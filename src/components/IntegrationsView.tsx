import { platformAccessHref } from "./owner-session";
import { Copy, KeyRound, Plus } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import {
  createApiKey, createFtpsDevice, deleteSecret, ftpsWriteThenReadBack, listApiKeys, listFtpsCredentials, listSecrets,
  revokeApiKey, revokeFtpsCredential, rotateFtpsDevice, setSecret,
  type ApiKey, type FileVisibility, type FtpsConnection, type FtpsCredentialList, type ProjectSecret,
} from "../deployments/client";
import { namedSecrets } from "./action-readiness";
import { apiKeyProblem, fileCurl, revokedKeyNotes, routeActions, routeCurl, secretProblem, type RouteAction } from "./app-access";
import { Badge, Button, Card, Dialog, EmptyState, Field, Input, LoadingBlock, Select } from "./ui";
import { useLoadingHold } from "./loading-hold";
import { message, signedOut, useConfirm } from "./useConfirm";
import "./app-access.css";

type Props = {
  projectId: string;
  deploymentId?: string;
  deploymentUrl?: string;
  actionsSource?: string;
};

function SignedOut() {
  return <p className="access-note">Sign in with <a href={platformAccessHref(window.location.pathname)}>owner sign-in</a> to manage signing secrets and API keys.</p>;
}

export function IntegrationsView({ projectId, deploymentId, deploymentUrl, actionsSource }: Props) {
  const [ownerSignedOut, setOwnerSignedOut] = useState(false);
  const onSignedOut = useCallback(() => setOwnerSignedOut(true), []);
  const routes = routeActions(actionsSource);
  return (
    <section className="access-view" aria-label="Integrations">
      <p className="access-intro">Other programs reach this app on its HTTP routes. A service proves itself with a signature made with a secret; a program you write sends an API key. Neither is an app account or an owner.</p>
      {ownerSignedOut ? <Card className="access-card"><SignedOut /></Card> : <>
        <SecretsCard projectId={projectId} actionsSource={actionsSource} onSignedOut={onSignedOut} />
        {deploymentId && deploymentUrl
          ? <><ApiKeysCard deploymentId={deploymentId} deploymentUrl={deploymentUrl} routes={routes} onSignedOut={onSignedOut} /><FtpsCard deploymentId={deploymentId} onSignedOut={onSignedOut} /></>
          : <Card className="access-card"><EmptyState icon={<KeyRound size={24} />} title="API keys need a published app" description="A key belongs to one published app and calls its routes. Deploy the app, then create a key here." /></Card>}
      </>}
    </section>
  );
}

function FtpsCard({ deploymentId, onSignedOut }: { deploymentId: string; onSignedOut: () => void }) {
  const [listed, setListed] = useState<FtpsCredentialList | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [destination, setDestination] = useState("");
  const [label, setLabel] = useState("");
  const [saving, setSaving] = useState(false);
  const [revealed, setRevealed] = useState<FtpsConnection | null>(null);
  const confirm = useConfirm(onSignedOut);

  useEffect(() => {
    let current = true;
    listFtpsCredentials(deploymentId).then((answer) => {
      if (!current) return;
      setListed(answer);
      setError("");
      const firstDevice = Object.entries(answer.destinations).find(([, item]) => item.kind === "device")?.[0] ?? "";
      setDestination((value) => value && answer.destinations[value]?.kind === "device" ? value : firstDevice);
    }, (cause) => {
      if (!current) return;
      if (signedOut(cause)) onSignedOut();
      setError(message(cause));
    });
    return () => { current = false; };
  }, [deploymentId, onSignedOut]);

  const issue = async (request: () => Promise<FtpsConnection>) => {
    setSaving(true);
    setError("");
    setNotice("");
    try {
      const answer = await ftpsWriteThenReadBack(request, setRevealed, () => listFtpsCredentials(deploymentId));
      setListed(answer);
      setError("");
    } catch (cause) {
      if (signedOut(cause)) onSignedOut();
      setError(message(cause));
    } finally {
      setSaving(false);
    }
  };

  const create = () => {
    const cleanLabel = label.trim();
    if (!destination || !cleanLabel) {
      setError("Choose a device destination and enter a label.");
      return;
    }
    void issue(() => createFtpsDevice(deploymentId, destination, cleanLabel).then((connection) => {
      setLabel("");
      return connection;
    }));
  };

  const devices = Object.entries(listed?.destinations ?? {}).filter(([, item]) => item.kind === "device");
  const credentials = listed?.credentials ?? [];

  return (
    <Card className="access-card" role="group" aria-label="FTPS credentials">
      <h3>FTPS device credentials</h3>
      <p className="access-note">Issue a credential for a device destination. It can upload more than one file. Konae shows each password once.</p>
      {error && <div className="access-error" role="alert">{error}</div>}
      {notice && <p className="access-note" role="status">{notice}</p>}
      {!listed ? !error && <LoadingBlock label="Loading FTPS credentials" rows={2} /> : <>
          {devices.length === 0 && <p className="access-note">Publish a device destination in app/ftps.json to issue new credentials.</p>}
          {credentials.length === 0 ? devices.length > 0 && <p className="access-note">No device credentials.</p> : (
            <ul className="access-keys" aria-label="Device credentials">
              {credentials.map((credential) => (
                <li key={credential.credentialId}>
                  <div className="access-key-head">
                    <strong>{credential.label}</strong>
                    <code>{credential.credentialId}</code>
                    <small>{credential.destination}{credential.createdAt ? ` · Created ${new Date(credential.createdAt).toLocaleDateString()}` : ""}</small>
                    <span className="access-row-actions">
                      <Button size="sm" loading={saving} disabled={saving || listed.destinations[credential.destination]?.kind !== "device"} onClick={() => void issue(() => rotateFtpsDevice(deploymentId, credential.credentialId))}>Rotate</Button>
                      <Button size="sm" variant="ghost" disabled={saving} onClick={() => confirm.ask({
                        title: `Revoke ${credential.label}?`,
                        description: "This device will no longer be able to upload with this credential.",
                        action: "Revoke credential",
                        run: async () => {
                          const answer = await ftpsWriteThenReadBack(
                            () => revokeFtpsCredential(deploymentId, credential.credentialId),
                            () => {},
                            () => listFtpsCredentials(deploymentId),
                          );
                          setListed(answer);
                          setError("");
                          setNotice(`Revoked ${credential.label}.`);
                        },
                      })}>Revoke</Button>
                    </span>
                  </div>
                </li>
              ))}
            </ul>
          )}
          {devices.length > 0 && <fieldset className="access-create">
            <legend>New device credential</legend>
            <Field label="Destination"><Select value={destination} onChange={(event) => setDestination(event.target.value)}>{devices.map(([name, item]) => <option key={name} value={name}>{name}{item.folder ? ` · ${item.folder}` : ""}</option>)}</Select></Field>
            <Field label="Label" help="Name the device that will use this credential."><Input value={label} maxLength={80} onChange={(event) => setLabel(event.target.value)} /></Field>
            <div className="access-actions"><Button variant="primary" size="sm" loading={saving} disabled={saving || !destination || !label.trim()} onClick={create}><Plus size={13} /> Create device credential</Button></div>
          </fieldset>}
        </>}
      {confirm.dialog}
      {revealed && <Dialog title="Copy this FTPS password now" description="Konae cannot show this password again after you close this dialog." onClose={() => setRevealed(null)} footer={<Button variant="primary" onClick={() => setRevealed(null)}>I copied the password</Button>}>
        <div className="ui-dialog-body access-reveal">
          <CopyLine label="Host" text={revealed.host} />
          <CopyLine label="Port" text={String(revealed.port)} />
          <CopyLine label="Username" text={revealed.username} />
          <CopyLine label="Password" text={revealed.password} />
          {revealed.expiresAt && <p className="access-note">Expires {new Date(revealed.expiresAt).toLocaleString()}.</p>}
        </div>
      </Dialog>}
    </Card>
  );
}

function SecretsCard({ projectId, actionsSource, onSignedOut }: { projectId: string; actionsSource?: string; onSignedOut: () => void }) {
  const [secrets, setSecrets] = useState<ProjectSecret[] | null>(null);
  const secretsPending = useLoadingHold(!secrets);
  const [error, setError] = useState("");
  const [reload, setReload] = useState(0);
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [saving, setSaving] = useState(false);
  const [formError, setFormError] = useState("");
  const [notice, setNotice] = useState("");
  const confirm = useConfirm(onSignedOut);
  const usage = namedSecrets(actionsSource);

  useEffect(() => {
    let current = true;
    listSecrets(projectId).then((answer) => { if (current) { setSecrets(answer.secrets); setError(""); } }, (cause) => {
      if (!current) return;
      if (signedOut(cause)) onSignedOut();
      setError(message(cause));
    });
    return () => { current = false; };
  }, [projectId, reload, onSignedOut]);

  const set = new Set((secrets ?? []).map((secret) => secret.name));
  const missing = [...usage.keys()].filter((secret) => !set.has(secret)).sort();
  const problem = name || value ? secretProblem(name, value) : "";

  const save = async () => {
    const refusal = secretProblem(name, value);
    if (refusal) { setFormError(refusal); return; }
    setSaving(true);
    setFormError("");
    setNotice("");
    try {
      await setSecret(projectId, name, value);
      setNotice(`Saved ${name}. Its value cannot be read back.`);
      setName("");
      setValue("");
      setReload((count) => count + 1);
    } catch (cause) {
      if (signedOut(cause)) onSignedOut();
      setFormError(message(cause));
    } finally {
      setSaving(false);
    }
  };

  let list: ReactNode;
  if (error) list = <div className="access-error" role="alert">{error}</div>;
  else if (!secrets || secretsPending) list = <LoadingBlock label="Loading secrets" rows={2} />;
  else if (secrets.length === 0 && missing.length === 0) list = <p className="access-note">No signing secrets. A route that declares a signature names one in actions.json.</p>;
  else list = (
    <ul className="access-list" aria-label="Signing secrets">
      {missing.map((secret) => (
        <li key={secret}>
          <code>{secret}</code>
          <Badge tone="warning">Not set</Badge>
          <small>{usedBy(usage.get(secret)!.actions)} Publishing is refused until it is set.</small>
          <Button size="sm" onClick={() => { setName(secret); setValue(""); }}>Set</Button>
        </li>
      ))}
      {secrets.map((secret) => {
        const used = usage.get(secret.name);
        const callsOnly = secret.use === "call" && used?.route;
        return (
        <li key={secret.name}>
          <code>{secret.name}</code>
          {callsOnly ? <Badge tone="warning">Calls only</Badge> : <Badge tone="success">Set</Badge>}
          <small>{callsOnly
            ? `It is set for calls only, and a route signs with it. Publishing is refused until you replace it with a route secret of its own value.`
            : `${used ? `${usedBy(used.actions)} ` : ""}Updated ${new Date(secret.updatedAt).toLocaleDateString()}`}</small>
          <span className="access-row-actions">
            <Button size="sm" onClick={() => { setName(secret.name); setValue(""); }}>Replace</Button>
            <Button size="sm" variant="ghost" aria-label={`Delete secret ${secret.name}`} onClick={() => confirm.ask({
              title: `Delete ${secret.name}?`,
              description: "Deleting it is refused while the published app signs a route with it. Senders that still sign with it get 401.",
              action: "Delete secret",
              run: async () => { await deleteSecret(projectId, secret.name); setReload((count) => count + 1); },
            })}>Delete</Button>
          </span>
        </li>
        );
      })}
    </ul>
  );

  return (
    <Card className="access-card" role="group" aria-label="Signing secrets">
      <h3>Signing secrets</h3>
      <p className="access-note">A sender signs each request with the secret in the header the route declares. Konae shows names only, never values.</p>
      {list}
      <div className="access-form-row">
        <Field label="Secret name"><Input value={name} placeholder="orders_webhook" autoComplete="off" spellCheck={false} onChange={(event) => { setName(event.target.value); setFormError(""); }} /></Field>
        <Field label="Secret value" help="16 to 512 printable characters without spaces."><Input type="password" value={value} autoComplete="new-password" aria-invalid={Boolean(formError || problem) || undefined} onChange={(event) => { setValue(event.target.value); setFormError(""); }} /></Field>
      </div>
      {(formError || (problem && value)) && <div className="access-error" role="alert">{formError || problem}</div>}
      <div className="access-actions">
        <Button variant="primary" size="sm" loading={saving} disabled={!name || !value} onClick={() => void save()}>Save secret</Button>
        <span role="status">{notice}</span>
      </div>
      {confirm.dialog}
    </Card>
  );
}

function usedBy(actions: string[]): string {
  return `${actions.join(", ")} ${actions.length === 1 ? "uses" : "use"} it.`;
}

function ApiKeysCard({ deploymentId, deploymentUrl, routes, onSignedOut }: { deploymentId: string; deploymentUrl: string; routes: RouteAction[]; onSignedOut: () => void }) {
  const [keys, setKeys] = useState<ApiKey[] | null>(null);
  const keysPending = useLoadingHold(!keys);
  const [error, setError] = useState("");
  const [reload, setReload] = useState(0);
  const [name, setName] = useState("");
  const [actions, setActions] = useState<string[]>([]);
  const [files, setFiles] = useState<FileVisibility[]>([]);
  const [creating, setCreating] = useState(false);
  const [formError, setFormError] = useState("");
  const [created, setCreated] = useState<(ApiKey & { key: string }) | null>(null);
  const confirm = useConfirm(onSignedOut);

  useEffect(() => {
    let current = true;
    listApiKeys(deploymentId).then((answer) => { if (current) { setKeys(answer.keys); setError(""); } }, (cause) => {
      if (!current) return;
      if (signedOut(cause)) onSignedOut();
      setError(message(cause));
    });
    return () => { current = false; };
  }, [deploymentId, reload, onSignedOut]);

  const toggle = <T extends string>(list: T[], item: T) => list.includes(item) ? list.filter((value) => value !== item) : [...list, item];

  const create = async () => {
    const refusal = apiKeyProblem(name, actions, files);
    if (refusal) { setFormError(refusal); return; }
    setCreating(true);
    setFormError("");
    try {
      const key = await createApiKey(deploymentId, name.trim(), actions, files);
      setCreated(key);
      setName("");
      setActions([]);
      setFiles([]);
      setReload((count) => count + 1);
    } catch (cause) {
      if (signedOut(cause)) onSignedOut();
      setFormError(message(cause));
    } finally {
      setCreating(false);
    }
  };

  const live = (keys ?? []).filter((key) => !key.revokedAt);
  const revoked = (keys ?? []).filter((key) => key.revokedAt);

  return (
    <Card className="access-card" role="group" aria-label="API keys">
      <h3>API keys</h3>
      <p className="access-note">Send the key as <code>Authorization: Bearer lbk_…</code> to this app's address. It works on the routes and file visibilities it names, replaces a route's signature, and opens no editor or owner tool.</p>
      {error && <div className="access-error" role="alert">{error}</div>}
      {!keys || keysPending ? !error && <LoadingBlock label="Loading API keys" rows={2} /> : live.length === 0 ? <p className="access-note">No live keys.</p> : (
        <ul className="access-keys" aria-label="Live API keys">
          {live.map((key) => (
            <li key={key.keyId}>
              <div className="access-key-head">
                <strong>{key.name}</strong>
                <code>{key.keyId}</code>
                <small>Created {new Date(key.createdAt).toLocaleDateString()}</small>
                <Button size="sm" variant="ghost" aria-label={`Revoke key ${key.name}`} onClick={() => confirm.ask({
                  title: `Revoke ${key.name}?`,
                  description: "Every program that sends this key gets 401 from now on. A revoked key cannot come back.",
                  action: "Revoke key",
                  run: async () => { await revokeApiKey(deploymentId, key.keyId); setReload((count) => count + 1); },
                })}>Revoke</Button>
              </div>
              <KeyCommands keyValue={undefined} apiKey={key} routes={routes} deploymentUrl={deploymentUrl} />
            </li>
          ))}
        </ul>
      )}
      {revokedKeyNotes(revoked).map((note) => <p key={note} className="access-note">{note}</p>)}

      <fieldset className="access-create">
        <legend>New API key</legend>
        <Field label="Key name" help="Who or what uses it, for example Billing export."><Input value={name} maxLength={80} onChange={(event) => { setName(event.target.value); setFormError(""); }} /></Field>
        <div className="access-checks" role="group" aria-label="Routes the key may call">
          <span>Routes</span>
          {routes.length === 0 ? <small>No action declares an HTTP route. Add http to an anonymous action in actions.json and deploy.</small> : routes.map((route) => (
            <label key={route.name}><input type="checkbox" checked={actions.includes(route.name)} onChange={() => { setActions(toggle(actions, route.name)); setFormError(""); }} /> <code>{route.method} {route.path}</code> <small>{route.name}{route.signature ? ", signed" : ""}</small></label>
          ))}
        </div>
        <div className="access-checks" role="group" aria-label="Files the key may read">
          <span>Files</span>
          <label><input type="checkbox" checked={files.includes("public")} onChange={() => { setFiles(toggle(files, "public")); setFormError(""); }} /> Public files</label>
          <label><input type="checkbox" checked={files.includes("private")} onChange={() => { setFiles(toggle(files, "private")); setFormError(""); }} /> Private files <small>Reads every private file of this app.</small></label>
        </div>
        {formError && <div className="access-error" role="alert">{formError}</div>}
        <div className="access-actions"><Button variant="primary" size="sm" loading={creating} onClick={() => void create()}><Plus size={13} /> Create API key</Button></div>
      </fieldset>
      {confirm.dialog}
      {created && (
        <Dialog title="Copy this API key now" description="Konae stores only its hash. Once this closes, nobody can show the key again." onClose={() => setCreated(null)} footer={<Button variant="primary" onClick={() => setCreated(null)}>I copied the key</Button>}>
          <div className="ui-dialog-body access-reveal">
            <CopyLine label="API key" text={created.key} />
            <KeyCommands keyValue={created.key} apiKey={created} routes={routes} deploymentUrl={deploymentUrl} />
          </div>
        </Dialog>
      )}
    </Card>
  );
}

function KeyCommands({ keyValue, apiKey, routes, deploymentUrl }: { keyValue?: string; apiKey: Pick<ApiKey, "actions" | "files">; routes: RouteAction[]; deploymentUrl: string }) {
  const named = apiKey.actions.map((action) => routes.find((route) => route.name === action)).filter((route): route is RouteAction => Boolean(route));
  const gone = apiKey.actions.filter((action) => !routes.some((route) => route.name === action));
  return (
    <div className="access-commands">
      {named.map((route) => <CopyLine key={route.name} label={`${route.method} ${route.path}`} text={routeCurl(deploymentUrl, route, keyValue)} />)}
      {apiKey.files.length > 0 && <CopyLine label={`Read ${apiKey.files.join(" and ")} files`} text={fileCurl(deploymentUrl, keyValue)} />}
      {gone.length > 0 && <small>{gone.join(", ")} no longer has a route in these sources.</small>}
      {!keyValue && (named.length > 0 || apiKey.files.length > 0) && <small>Set KONAE_API_KEY to the key you copied when you created it.</small>}
    </div>
  );
}

export function CopyLine({ label, text }: { label: string; text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="access-copy">
      <span>{label}</span>
      <code>{text}</code>
      <Button size="sm" variant="ghost" aria-label={`Copy ${label}`} onClick={() => void navigator.clipboard?.writeText(text).then(() => setCopied(true), () => setCopied(false))}>
        <Copy size={13} /> {copied ? "Copied" : "Copy"}
      </Button>
    </div>
  );
}
