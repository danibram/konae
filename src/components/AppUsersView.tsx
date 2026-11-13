import { platformAccessHref } from "./owner-session";
import { MailWarning, RefreshCw, Send, TriangleAlert, Users } from "lucide-react";
import { useCallback, useEffect, useId, useState, type ReactNode } from "react";
import {
  createAppInvitations, listAppInvitations, listAppUsers, readMailStatus, readSignInMethods, readSignupPolicy, resendAppInvitation, resumeAppUser,
  revokeAppInvitation, enterAsAppUser, saveSignInMethods, saveSignupPolicy, setGoogleClientPart, signupModes, suspendAppUser, GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET,
  type AppInvitation, type AppUser, type AppUserState, type InvitationResult, type MailStatus, type SignInMethods, type SignupMode, type SignupPolicy,
} from "../deployments/client";
import type { DeploymentVisibility } from "../deployments/manifest";
import {
  batchKey, expiryText, formatDate, invitationResultMessage, invitationView, mailReadiness, parseEmailBatch, signupModeCopy,
  userStateView, visibilityCopy, MAX_INVITATION_BATCH, type PendingBatch,
} from "./app-access";
import { CopyLine } from "./IntegrationsView";
import { googleRefusal, redirectUriLabel, type GoogleField } from "./app-access";
import { Badge, Button, Card, EmptyState, Field, Input, LoadingBlock, Select, Switch, Textarea } from "./ui";
import { useLoadingHold } from "./loading-hold";
import { message, signedOut, useConfirm } from "./useConfirm";
import "./app-access.css";

type Props = {
  /** Secrets belong to the project; the app's Google client is two of them. */
  projectId?: string;
  deploymentId?: string;
  visibility: DeploymentVisibility;
  onSetVisibility?: (visibility: DeploymentVisibility) => void;
  fileAccess?: "app-users";
  onSetFileAccess?: (shared: boolean) => void;
};

const PAGE = 25;


function SignedOut() {
  return <p className="access-note">Sign in with <a href={platformAccessHref(window.location.pathname)}>owner sign-in</a> to manage this app's accounts.</p>;
}

/** App accounts, invitations and who may join, for the published app. Owners only. */
export function AppUsersView({ projectId, deploymentId, visibility, onSetVisibility, fileAccess, onSetFileAccess }: Props) {
  const [mail, setMail] = useState<MailStatus | null>(null);
  const [mailError, setMailError] = useState("");
  const [changed, setChanged] = useState(0);
  const [ownerSignedOut, setOwnerSignedOut] = useState(false);

  useEffect(() => {
    if (!deploymentId) return;
    let current = true;
    readMailStatus().then((status) => { if (current) setMail(status); }, (cause) => {
      if (!current) return;
      if (signedOut(cause)) setOwnerSignedOut(true);
      else setMailError(message(cause));
    });
    return () => { current = false; };
  }, [deploymentId]);

  const readiness = mailError ? { ready: false, reason: `Could not read the mail status: ${mailError}` } : mailReadiness(mail);
  const onChanged = useCallback(() => setChanged((value) => value + 1), []);
  const onSignedOut = useCallback(() => setOwnerSignedOut(true), []);

  return (
    <section className="access-view" aria-label="App users">
      <Card className="access-card" role="group" aria-label="Who can open and join">
        <h3>Who can open and join</h3>
        <Field label="Who can open the app" help={`${visibilityCopy[visibility].sentence} ${onSetVisibility ? "It applies when you deploy." : "It changes in the app's folder, with a publish."}`}>
          <Select value={visibility} disabled={!onSetVisibility} onChange={(event) => onSetVisibility?.(event.target.value === "private" ? "private" : "public")}>
            <option value="public">Anyone with the link</option>
            <option value="private">People with an app account</option>
          </Select>
        </Field>
        <Field label="Who can read uploaded files" help={onSetFileAccess ? "Deploy to apply this to existing and new files. Choosing per-file access restores earlier visibility settings. Only the uploader can delete a file." : "It changes in the app's folder, with a publish. Only the uploader can delete a file."}>
          <Select value={fileAccess ?? "per-file"} disabled={!onSetFileAccess} onChange={(event) => onSetFileAccess?.(event.target.value === "app-users")}>
            <option value="per-file">Each file's visibility</option>
            <option value="app-users">All signed-in app users</option>
          </Select>
        </Field>
        {deploymentId
          ? ownerSignedOut ? <SignedOut /> : <SignupPolicyControl deploymentId={deploymentId} onSignedOut={onSignedOut} onSaved={onChanged} />
          : <p className="access-note">You choose who can create an app account when you deploy. Invitation only is the choice for a private app.</p>}
      </Card>
      {deploymentId && projectId && !ownerSignedOut && <GoogleSignIn projectId={projectId} deploymentId={deploymentId} onSignedOut={onSignedOut} />}
      {!deploymentId ? (
        <Card className="access-card">
          <EmptyState icon={<Users size={24} />} title="No app accounts yet" description="People create app accounts on the published app, never in this editor. Deploy the app to invite people and see their accounts here." />
        </Card>
      ) : ownerSignedOut ? null : (
        <>
          {!readiness.ready && <p className="access-mail" role="status"><MailWarning size={15} /><span>{readiness.reason}</span></p>}
          <InviteForm deploymentId={deploymentId} mailReady={readiness.ready} mailReason={readiness.reason} onInvited={onChanged} onSignedOut={onSignedOut} />
          <InvitationList deploymentId={deploymentId} mailReady={readiness.ready} mailReason={readiness.reason} changed={changed} onChanged={onChanged} onSignedOut={onSignedOut} />
          <AccountList deploymentId={deploymentId} changed={changed} onSignedOut={onSignedOut} />
        </>
      )}
    </section>
  );
}

function SignupPolicyControl({ deploymentId, onSignedOut, onSaved }: { deploymentId: string; onSignedOut: () => void; onSaved: () => void }) {
  const [policy, setPolicy] = useState<SignupPolicy | null>(null);
  const policyPending = useLoadingHold(!policy);
  const [mode, setMode] = useState<SignupMode>("invitation");
  const [entries, setEntries] = useState("");
  const [loadError, setLoadError] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    let current = true;
    setPolicy(null);
    readSignupPolicy(deploymentId).then((value) => {
      if (!current) return;
      setPolicy(value);
      setMode(value.mode);
      setEntries(value.allowlist.join("\n"));
    }, (cause) => {
      if (!current) return;
      if (signedOut(cause)) onSignedOut();
      else setLoadError(message(cause));
    });
    return () => { current = false; };
  }, [deploymentId, onSignedOut]);

  if (loadError) return <div className="access-error" role="alert">{loadError}</div>;
  if (!policy || policyPending) return <LoadingBlock label="Loading the signup policy" rows={2} />;

  const list = entries.split(/[\n,]/).map((entry) => entry.trim()).filter(Boolean);
  const unchanged = mode === policy.mode && (mode !== "allowlist" || list.join("\n") === policy.allowlist.join("\n"));
  const save = async () => {
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      const next = await saveSignupPolicy(deploymentId, mode, mode === "allowlist" ? list : policy.allowlist);
      setPolicy(next);
      setMode(next.mode);
      setEntries(next.allowlist.join("\n"));
      setSaved(true);
      onSaved();
    } catch (cause) {
      if (signedOut(cause)) onSignedOut();
      setError(message(cause));
    } finally {
      setSaving(false);
    }
  };

  return <>
    <Field label="Who can create an app account" help={signupModeCopy[mode].sentence}>
      <Select value={mode} onChange={(event) => { setMode(event.target.value as SignupMode); setSaved(false); }}>
        {signupModes.filter((value) => !signupModeCopy[value].advanced).map((value) => <option key={value} value={value}>{signupModeCopy[value].label}</option>)}
        <optgroup label="Advanced">
          {signupModes.filter((value) => signupModeCopy[value].advanced).map((value) => <option key={value} value={value}>{signupModeCopy[value].label}</option>)}
        </optgroup>
      </Select>
    </Field>
    {mode === "allowlist" && <Field label="Allowed emails and domains" help="One per line, for example ana@example.com or @example.com. Up to 100 entries. Without verified email, whoever signs up first with a listed address gets it.">
      <Textarea rows={4} value={entries} aria-invalid={Boolean(error) || undefined} onChange={(event) => { setEntries(event.target.value); setSaved(false); }} />
    </Field>}
    {error && <div className="access-error" role="alert">{error}</div>}
    <div className="access-actions">
      <Button variant="primary" size="sm" loading={saving} disabled={unchanged} onClick={() => void save()}>Save signup policy</Button>
      <span role="status">{saved ? "Saved. The published app uses it now." : ""}</span>
    </div>
  </>;
}

/**
 * Continue with Google on the sign-in page. The owner makes a Google OAuth
 * client for this app in their own Google console, registers the redirect
 * URIs shown here, pastes its client ID and secret, and turns Google on.
 * The switch stays off until both halves are set, as the Kernel refuses it.
 */
function GoogleSignIn({ projectId, deploymentId, onSignedOut }: { projectId: string; deploymentId: string; onSignedOut: () => void }) {
  const [methods, setMethods] = useState<SignInMethods | null>(null);
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [invalid, setInvalid] = useState<GoogleField | undefined>();
  const [status, setStatus] = useState("");
  const pending = useLoadingHold(!methods && !error);

  // The Kernel checks the values; the card shows its sentence by the field it is about.
  const fail = useCallback((cause: unknown, saving?: GoogleField) => {
    if (signedOut(cause)) { onSignedOut(); return; }
    const refused = googleRefusal(cause, saving);
    setError(refused?.message ?? message(cause));
    setInvalid(refused?.field);
  }, [onSignedOut]);

  useEffect(() => {
    let current = true;
    setMethods(null);
    readSignInMethods(deploymentId).then((value) => { if (current) setMethods(value); }, (cause) => { if (current) fail(cause); });
    return () => { current = false; };
  }, [deploymentId, fail]);

  const saveClient = async () => {
    setSaving(true);
    setError("");
    setInvalid(undefined);
    setStatus("");
    let saving: GoogleField | undefined;
    try {
      if (clientId.trim()) {
        saving = "clientId";
        await setGoogleClientPart(projectId, GOOGLE_CLIENT_ID, clientId.trim());
        setClientId("");
      }
      if (clientSecret.trim()) {
        saving = "clientSecret";
        await setGoogleClientPart(projectId, GOOGLE_CLIENT_SECRET, clientSecret.trim());
        setClientSecret("");
      }
      saving = undefined;
      setMethods(await readSignInMethods(deploymentId));
      setStatus("Saved. Konae keeps the values and never shows them again.");
    } catch (cause) {
      fail(cause, saving);
      // A client ID saved before the secret was refused still counts.
      if (saving === "clientSecret") readSignInMethods(deploymentId).then(setMethods, () => {});
    } finally {
      setSaving(false);
    }
  };

  const toggle = async (next: boolean) => {
    setSaving(true);
    setError("");
    setInvalid(undefined);
    setStatus("");
    try {
      setMethods(await saveSignInMethods(deploymentId, next));
      setStatus(next ? "Google is on. The sign-in page shows Continue with Google now." : "Google is off. The sign-in page shows email and password only.");
    } catch (cause) {
      fail(cause);
    } finally {
      setSaving(false);
    }
  };

  let body: ReactNode;
  if (!methods) body = pending ? <LoadingBlock label="Loading Google sign-in" rows={2} /> : null;
  else {
    const { clientIdSet, clientSecretSet } = methods.googleClient;
    const ready = clientIdSet && clientSecretSet;
    const on = methods.methods.includes("google");
    const setLabel = (set: boolean) => <Badge tone={set ? "success" : "neutral"}>{set ? "Set" : "Not set"}</Badge>;
    body = <>
      {methods.notice && <p className="access-mail" role="status"><TriangleAlert size={15} /><span>{methods.notice}</span></p>}
      <ol className="access-steps">
        <li>
          <strong>Make a Google client for this app.</strong>
          <p className="access-note">In the Google Cloud console, open APIs &amp; Services, then Credentials, and create an OAuth client ID of type Web application. Paste {methods.redirectUris.length > 1 ? "each address" : "this address"} under Authorized redirect URIs.</p>
          {methods.redirectUris.map((uri, index) => <CopyLine key={uri} label={redirectUriLabel(uri, index)} text={uri} />)}
        </li>
        <li>
          <strong>Paste the client ID and secret Google shows you.</strong>
          <div className="access-form-row">
            <Field label="Client ID" help={clientIdSet ? "Set. Paste a new one to replace it." : "Ends with .apps.googleusercontent.com"}>
              <Input value={clientId} aria-invalid={invalid === "clientId" || undefined} autoComplete="off" spellCheck={false} placeholder={clientIdSet ? "Set" : "123456-abc.apps.googleusercontent.com"} onChange={(event) => setClientId(event.target.value)} />
            </Field>
            <Field label="Client secret" help={clientSecretSet ? "Set. Paste a new one to replace it." : "Starts with GOCSPX- for a new client"}>
              <Input type="password" value={clientSecret} aria-invalid={invalid === "clientSecret" || undefined} autoComplete="off" placeholder={clientSecretSet ? "Set" : "GOCSPX-..."} onChange={(event) => setClientSecret(event.target.value)} />
            </Field>
          </div>
          <div className="access-actions">
            <span>Client ID {setLabel(clientIdSet)} Client secret {setLabel(clientSecretSet)}</span>
            <Button variant="primary" size="sm" loading={saving} disabled={!clientId.trim() && !clientSecret.trim()} onClick={() => void saveClient()}>Save client</Button>
          </div>
        </li>
        <li>
          <strong>Turn it on.</strong>
          <Switch checked={on} disabled={saving || (!ready && !on)} label="Continue with Google" onChange={(next) => void toggle(next)} />
          {!ready && !on && <p className="access-note access-disabled-reason">Set the client ID and the client secret first.</p>}
        </li>
      </ol>
      {error && <div className="access-error" role="alert">{error}</div>}
      {status && <div className="access-actions"><span role="status">{status}</span></div>}
      <p className="access-note">A person who continues with Google reaches the app account with the same email, whether it was made with a password or with Google. Google proves only Gmail addresses and addresses of a Google Workspace domain; anyone else signs in with email and password. The signup policy above still decides who may create an account.</p>
    </>;
  }

  return (
    <Card className="access-card" role="group" aria-label="Sign in with Google">
      <h3>Sign in with Google</h3>
      {body}
      {!methods && error && <div className="access-error" role="alert">{error}</div>}
    </Card>
  );
}

function InviteForm({ deploymentId, mailReady, mailReason, onInvited, onSignedOut }: { deploymentId: string; mailReady: boolean; mailReason: string; onInvited: () => void; onSignedOut: () => void }) {
  const [text, setText] = useState("");
  const [pending, setPending] = useState<PendingBatch | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState("");
  const [results, setResults] = useState<InvitationResult[]>([]);
  const reasonId = useId();
  const batch = parseEmailBatch(text);

  const invite = async () => {
    if (!mailReady || sending || batch.emails.length === 0 || batch.tooMany) return;
    // The same addresses after a failed call keep their key, so a retry
    // creates and sends nothing twice.
    const next = batchKey(pending, batch.emails);
    setPending(next);
    setSending(true);
    setError("");
    try {
      // Sorted, so a retry after reordering lines sends the arguments its key was made for.
      const answer = await createAppInvitations(deploymentId, [...batch.emails].sort(), next.key);
      setResults(answer.results);
      setPending(null);
      setText("");
      onInvited();
    } catch (cause) {
      if (signedOut(cause)) onSignedOut();
      setError(message(cause));
    } finally {
      setSending(false);
    }
  };

  const help = batch.tooMany
    ? `Invite at most ${MAX_INVITATION_BATCH} people at a time.`
    : "One address per line or separated by commas. Each person gets a link to create an app account; it works once and expires after seven days.";

  return (
    <Card className="access-card" role="group" aria-label="Invite people">
      <h3>Invite people</h3>
      <Field label="Email addresses" help={help}>
        <Textarea rows={3} value={text} aria-invalid={batch.tooMany || undefined} placeholder="ana@example.com" onChange={(event) => setText(event.target.value)} />
      </Field>
      {error && <div className="access-error" role="alert">{error}{pending && " Sending the same addresses again is safe; nothing is sent twice."}</div>}
      <div className="access-actions">
        <Button variant="primary" size="sm" loading={sending} disabled={!mailReady || batch.emails.length === 0 || batch.tooMany} aria-describedby={mailReady ? undefined : reasonId} onClick={() => void invite()}>
          <Send size={13} /> {batch.emails.length > 1 ? `Invite ${batch.emails.length} people` : "Send invitation"}
        </Button>
        {!mailReady && <span id={reasonId} className="access-disabled-reason">Off: {mailReason}</span>}
      </div>
      {results.length > 0 && (
        <ul className="access-results" aria-label="Invitation results">
          {results.map((result) => {
            const view = invitationResultMessage(result);
            return <li key={result.email}><strong>{result.email}</strong><Badge tone={view.tone}>{view.label}</Badge><small>{view.text}</small></li>;
          })}
        </ul>
      )}
    </Card>
  );
}

function InvitationList({ deploymentId, mailReady, mailReason, changed, onChanged, onSignedOut }: { deploymentId: string; mailReady: boolean; mailReason: string; changed: number; onChanged: () => void; onSignedOut: () => void }) {
  const [rows, setRows] = useState<AppInvitation[] | null>(null);
  const invitationsPending = useLoadingHold(!rows);
  const [next, setNext] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [loadingMore, setLoadingMore] = useState(false);
  const [resending, setResending] = useState<Record<string, string>>({});
  const [notice, setNotice] = useState("");
  const confirm = useConfirm(onSignedOut);
  const reasonId = useId();

  useEffect(() => {
    let current = true;
    listAppInvitations(deploymentId, { limit: PAGE }).then((page) => {
      if (!current) return;
      setRows(page.invitations);
      setNext(page.nextCursor);
      setError("");
    }, (cause) => {
      if (!current) return;
      if (signedOut(cause)) onSignedOut();
      setError(message(cause));
    });
    return () => { current = false; };
  }, [deploymentId, changed, onSignedOut]);

  const more = async () => {
    if (!next) return;
    setLoadingMore(true);
    try {
      const page = await listAppInvitations(deploymentId, { limit: PAGE, cursor: next });
      setRows((current) => [...(current ?? []), ...page.invitations]);
      setNext(page.nextCursor);
    } catch (cause) {
      setError(message(cause));
    } finally {
      setLoadingMore(false);
    }
  };

  const resend = async (invitation: AppInvitation) => {
    if (!mailReady) return;
    // One key per invitation until the resend succeeds, so a retry sends one email.
    const key = resending[invitation.invitationId] ?? `resend-${crypto.randomUUID()}`;
    setResending((current) => ({ ...current, [invitation.invitationId]: key }));
    setNotice("");
    try {
      await resendAppInvitation(deploymentId, invitation.invitationId, key);
      setNotice(`Sent a new link to ${invitation.email}. The earlier link no longer works.`);
      setResending(({ [invitation.invitationId]: _, ...rest }) => rest);
      onChanged();
    } catch (cause) {
      if (signedOut(cause)) onSignedOut();
      setNotice(`${invitation.email}: ${message(cause)}`);
      setResending(({ [invitation.invitationId]: _, ...rest }) => rest);
    }
  };

  let body: ReactNode;
  if (error && !rows) body = <div className="access-error" role="alert">{error}</div>;
  else if (!rows || invitationsPending) body = <LoadingBlock label="Loading invitations" />;
  else if (rows.length === 0) body = <p className="access-note">No invitations yet. Invite someone above and they appear here with their state.</p>;
  else body = (
    <div className="access-table-wrap">
      <table className="access-table">
        <thead><tr><th scope="col">Email</th><th scope="col">State</th><th scope="col">Email sent</th><th scope="col">Link</th><th scope="col"><span className="sr-only">Actions</span></th></tr></thead>
        <tbody>
          {rows.map((invitation) => {
            const view = invitationView(invitation);
            return (
              <tr key={invitation.invitationId}>
                <td className="access-email">{invitation.email}</td>
                <td><Badge tone={view.tone}>{view.label}</Badge></td>
                <td><Badge tone={view.delivery.tone}>{view.delivery.label}</Badge> <small>{view.sends}</small></td>
                <td><small>{expiryText(invitation)}</small></td>
                <td className="access-row-actions">
                  {view.canResend && <Button size="sm" disabled={!mailReady} loading={Boolean(resending[invitation.invitationId])} aria-describedby={mailReady ? undefined : reasonId} aria-label={`Resend invitation to ${invitation.email}`} onClick={() => void resend(invitation)}>Resend</Button>}
                  {view.canRevoke && <Button size="sm" variant="ghost" aria-label={`Revoke invitation to ${invitation.email}`} onClick={() => confirm.ask({
                    title: "Revoke this invitation?",
                    description: `The link sent to ${invitation.email} stops working. You can invite the address again later.`,
                    action: "Revoke invitation",
                    run: async () => { await revokeAppInvitation(deploymentId, invitation.invitationId); onChanged(); },
                  })}>Revoke</Button>}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );

  return (
    <Card className="access-card" role="group" aria-label="Invitations">
      <header className="access-card-head">
        <h3>Invitations</h3>
        <Button size="sm" variant="ghost" aria-label="Refresh invitations" onClick={onChanged}><RefreshCw size={13} /></Button>
      </header>
      {!mailReady && rows && rows.length > 0 && <span id={reasonId} className="sr-only">Resend is off: {mailReason}</span>}
      {body}
      {error && rows && <div className="access-error" role="alert">{error}</div>}
      <p className="access-note" role="status">{notice}</p>
      {next && <Button size="sm" loading={loadingMore} onClick={() => void more()}>Show more invitations</Button>}
      {confirm.dialog}
    </Card>
  );
}

const stateFilters: Array<{ value: "" | AppUserState; label: string }> = [
  { value: "", label: "All app accounts" },
  { value: "active", label: "Active" },
  { value: "pending", label: "Pending" },
  { value: "suspended", label: "Suspended" },
];

function AccountList({ deploymentId, changed, onSignedOut }: { deploymentId: string; changed: number; onSignedOut: () => void }) {
  const [filter, setFilter] = useState<"" | AppUserState>("");
  // Each entry is the cursor that opens a page; the first page has none.
  const [cursors, setCursors] = useState<Array<string | undefined>>([undefined]);
  const [rows, setRows] = useState<AppUser[] | null>(null);
  const accountsPending = useLoadingHold(!rows);
  const [next, setNext] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [reload, setReload] = useState(0);
  const confirm = useConfirm(onSignedOut);
  const cursor = cursors[cursors.length - 1];

  useEffect(() => {
    let current = true;
    listAppUsers(deploymentId, { limit: PAGE, ...(cursor ? { cursor } : {}), ...(filter ? { state: filter } : {}) }).then((page) => {
      if (!current) return;
      setRows(page.users);
      setNext(page.nextCursor);
      setError("");
    }, (cause) => {
      if (!current) return;
      if (signedOut(cause)) onSignedOut();
      setError(message(cause));
    });
    return () => { current = false; };
  }, [deploymentId, cursor, filter, changed, reload, onSignedOut]);

  const resume = async (user: AppUser) => {
    try {
      await resumeAppUser(deploymentId, user.userId);
      setReload((value) => value + 1);
    } catch (cause) {
      if (signedOut(cause)) onSignedOut();
      setError(message(cause));
    }
  };

  // The tab opens before the call, so the browser treats it as the click's own window.
  const enter = async (user: AppUser) => {
    const tab = window.open("", "_blank");
    try {
      const { url } = await enterAsAppUser(deploymentId, user.userId);
      if (tab) tab.location.href = url;
      else window.location.assign(url);
    } catch (cause) {
      tab?.close();
      if (signedOut(cause)) onSignedOut();
      setError(message(cause));
    }
  };

  return (
    <Card className="access-card" role="group" aria-label="App accounts">
      <header className="access-card-head">
        <h3>App accounts</h3>
        <Select aria-label="Show app accounts" value={filter} onChange={(event) => { setFilter(event.target.value as "" | AppUserState); setCursors([undefined]); }}>
          {stateFilters.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        </Select>
      </header>
      <p className="access-note">People who use this app. An app account cannot open this editor. The ID under each email is what an action binds as actor.id, and what an app's own roles table stores. Open as signs you in to the app as that person in a new tab; your audit log records it, and Usage and Signals do not count it.</p>
      {error && <div className="access-error" role="alert">{error}</div>}
      {!rows || accountsPending ? <LoadingBlock label="Loading app accounts" rows={4} /> : rows.length === 0 ? (
        <p className="access-note">{filter ? `No ${userStateView[filter].label.toLowerCase()} app accounts.` : "No app accounts yet. People appear here once they sign up or you invite them."}</p>
      ) : (
        <div className="access-table-wrap">
          <table className="access-table">
            <thead><tr><th scope="col">Email</th><th scope="col">State</th><th scope="col">Since</th><th scope="col"><span className="sr-only">Actions</span></th></tr></thead>
            <tbody>
              {rows.map((user) => (
                <tr key={user.userId}>
                  <td className="access-email">{user.email}<small className="access-user-id" title="The account ID an action binds as actor.id">{user.userId}</small></td>
                  <td><Badge tone={userStateView[user.state].tone} title={userStateView[user.state].sentence}>{userStateView[user.state].label}</Badge>{user.state !== "pending" && !user.verified && <small> Email not verified</small>}</td>
                  <td><small>{formatDate(user.createdAt)}</small></td>
                  <td className="access-row-actions">
                    {user.state === "active" && <Button size="sm" variant="ghost" aria-label={`Open the app as ${user.email}`} onClick={() => void enter(user)}>Open as</Button>}
                    {user.state === "suspended"
                      ? <Button size="sm" aria-label={`Resume ${user.email}`} onClick={() => void resume(user)}>Resume</Button>
                      : <Button size="sm" variant="ghost" aria-label={`Suspend ${user.email}`} onClick={() => confirm.ask({
                        title: "Suspend this app account?",
                        description: `${user.email} is signed out now and cannot sign in until you resume the account. Its rows and password stay.`,
                        action: "Suspend app account",
                        run: async () => { await suspendAppUser(deploymentId, user.userId); setReload((value) => value + 1); },
                      })}>Suspend</Button>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {(cursors.length > 1 || next) && (
        <nav className="access-actions" aria-label="App account pages">
          <Button size="sm" disabled={cursors.length === 1} onClick={() => setCursors((current) => current.slice(0, -1))}>Previous</Button>
          <span>Page {cursors.length}</span>
          <Button size="sm" disabled={!next} onClick={() => next && setCursors((current) => [...current, next])}>Next</Button>
        </nav>
      )}
      {confirm.dialog}
    </Card>
  );
}

