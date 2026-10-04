import { Copy, KeyRound, Plus } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import {
  createApiKey, deleteSecret, listApiKeys, listSecrets, revokeApiKey, setSecret,
  type ApiKey, type FileVisibility, type ProjectSecret,
} from "../deployments/client";
import { apiKeyProblem, fileCurl, revokedKeyNotes, routeActions, routeCurl, secretProblem, secretUsage, type RouteAction } from "./app-access";
import { Badge, Button, Card, Dialog, EmptyState, Field, Input } from "./ui";
import { message, signedOut, useConfirm } from "./useConfirm";
import "./app-access.css";

type Props = {
  projectId: string;
  deploymentId?: string;
  deploymentUrl?: string;
  actionsSource?: string;
};

const SIGNED_OUT = <p className="access-note">Sign in with <a href="/_littlebox/owner/login">owner sign-in</a> to manage signing secrets and API keys.</p>;

/** Signing secrets for signed routes and API keys for programs the owner writes. */
export function IntegrationsView({ projectId, deploymentId, deploymentUrl, actionsSource }: Props) {
  const [ownerSignedOut, setOwnerSignedOut] = useState(false);
  const onSignedOut = useCallback(() => setOwnerSignedOut(true), []);
  const routes = routeActions(actionsSource);
  return (
    <section className="access-view" aria-label="Integrations">
      <p className="access-intro">Other programs reach this app on its HTTP routes. A service proves itself with a signature made with a secret; a program you write sends an API key. Neither is an app account or an owner.</p>
      {ownerSignedOut ? <Card className="access-card">{SIGNED_OUT}</Card> : <>
        <SecretsCard projectId={projectId} actionsSource={actionsSource} onSignedOut={onSignedOut} />
        {deploymentId && deploymentUrl
          ? <ApiKeysCard deploymentId={deploymentId} deploymentUrl={deploymentUrl} routes={routes} onSignedOut={onSignedOut} />
          : <Card className="access-card"><EmptyState icon={<KeyRound size={24} />} title="API keys need a published app" description="A key belongs to one published app and calls its routes. Deploy the app, then create a key here." /></Card>}
      </>}
    </section>
  );
}

function SecretsCard({ projectId, actionsSource, onSignedOut }: { projectId: string; actionsSource?: string; onSignedOut: () => void }) {
  const [secrets, setSecrets] = useState<ProjectSecret[] | null>(null);
  const [error, setError] = useState("");
  const [reload, setReload] = useState(0);
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [saving, setSaving] = useState(false);
  const [formError, setFormError] = useState("");
  const [notice, setNotice] = useState("");
  const confirm = useConfirm(onSignedOut);
  const usage = secretUsage(actionsSource);

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
  else if (!secrets) list = <p className="access-note">Loading secrets.</p>;
  else if (secrets.length === 0 && missing.length === 0) list = <p className="access-note">No signing secrets. A route that declares a signature names one in actions.json.</p>;
  else list = (
    <ul className="access-list" aria-label="Signing secrets">
      {missing.map((secret) => (
        <li key={secret}>
          <code>{secret}</code>
          <Badge tone="warning">Not set</Badge>
          <small>{usage.get(secret)!.join(", ")} {usage.get(secret)!.length === 1 ? "signs" : "sign"} with it. Publishing is refused until it is set.</small>
          <Button size="sm" onClick={() => { setName(secret); setValue(""); }}>Set</Button>
        </li>
      ))}
      {secrets.map((secret) => (
        <li key={secret.name}>
          <code>{secret.name}</code>
          <Badge tone="success">Set</Badge>
          <small>{usage.has(secret.name) ? `Used by ${usage.get(secret.name)!.join(", ")}. ` : ""}Updated {new Date(secret.updatedAt).toLocaleDateString()}</small>
          <span className="access-row-actions">
            <Button size="sm" onClick={() => { setName(secret.name); setValue(""); }}>Replace</Button>
            <Button size="sm" variant="ghost" aria-label={`Delete secret ${secret.name}`} onClick={() => confirm.ask({
              title: `Delete ${secret.name}?`,
              description: "The Kernel refuses while the published app signs a route with it. Senders that still sign with it get 401.",
              action: "Delete secret",
              run: async () => { await deleteSecret(projectId, secret.name); setReload((count) => count + 1); },
            })}>Delete</Button>
          </span>
        </li>
      ))}
    </ul>
  );

  return (
    <Card className="access-card" role="group" aria-label="Signing secrets">
      <h3>Signing secrets</h3>
      <p className="access-note">A sender signs each request with the secret in the header the route declares. Littlebox shows names only, never values.</p>
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

function ApiKeysCard({ deploymentId, deploymentUrl, routes, onSignedOut }: { deploymentId: string; deploymentUrl: string; routes: RouteAction[]; onSignedOut: () => void }) {
  const [keys, setKeys] = useState<ApiKey[] | null>(null);
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
      {!keys ? !error && <p className="access-note">Loading API keys.</p> : live.length === 0 ? <p className="access-note">No live keys.</p> : (
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
        <Dialog title="Copy this API key now" description="Littlebox stores only its hash. Once this closes, nobody can show the key again." onClose={() => setCreated(null)} footer={<Button variant="primary" onClick={() => setCreated(null)}>I copied the key</Button>}>
          <div className="ui-dialog-body access-reveal">
            <CopyLine label="API key" text={created.key} />
            <KeyCommands keyValue={created.key} apiKey={created} routes={routes} deploymentUrl={deploymentUrl} />
          </div>
        </Dialog>
      )}
    </Card>
  );
}

/** The curl that calls each route and file visibility the key names, ready to copy. */
function KeyCommands({ keyValue, apiKey, routes, deploymentUrl }: { keyValue?: string; apiKey: Pick<ApiKey, "actions" | "files">; routes: RouteAction[]; deploymentUrl: string }) {
  const named = apiKey.actions.map((action) => routes.find((route) => route.name === action)).filter((route): route is RouteAction => Boolean(route));
  const gone = apiKey.actions.filter((action) => !routes.some((route) => route.name === action));
  return (
    <div className="access-commands">
      {named.map((route) => <CopyLine key={route.name} label={`${route.method} ${route.path}`} text={routeCurl(deploymentUrl, route, keyValue)} />)}
      {apiKey.files.length > 0 && <CopyLine label={`Read ${apiKey.files.join(" and ")} files`} text={fileCurl(deploymentUrl, keyValue)} />}
      {gone.length > 0 && <small>{gone.join(", ")} no longer has a route in these sources.</small>}
      {!keyValue && (named.length > 0 || apiKey.files.length > 0) && <small>Set LITTLEBOX_API_KEY to the key you copied when you created it.</small>}
    </div>
  );
}

function CopyLine({ label, text }: { label: string; text: string }) {
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
