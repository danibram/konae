import { MailWarning, RefreshCw, Send, Users } from "lucide-react";
import { useCallback, useEffect, useId, useState, type ReactNode } from "react";
import {
  createAppInvitations, listAppInvitations, listAppUsers, readMailStatus, readSignupPolicy, resendAppInvitation, resumeAppUser,
  revokeAppInvitation, saveSignupPolicy, signupModes, suspendAppUser,
  type AppInvitation, type AppUser, type AppUserState, type InvitationResult, type MailStatus, type SignupMode, type SignupPolicy,
} from "../deployments/client";
import type { DeploymentVisibility } from "../deployments/manifest";
import {
  batchKey, expiryText, formatDate, invitationResultMessage, invitationView, mailReadiness, parseEmailBatch, signupModeCopy,
  userStateView, visibilityCopy, MAX_INVITATION_BATCH, type PendingBatch,
} from "./app-access";
import { Badge, Button, Card, EmptyState, Field, Select, Textarea } from "./ui";
import { message, signedOut, useConfirm } from "./useConfirm";
import "./app-access.css";

type Props = {
  deploymentId?: string;
  visibility: DeploymentVisibility;
  onSetVisibility?: (visibility: DeploymentVisibility) => void;
};

const PAGE = 25;


const SIGNED_OUT = <p className="access-note">Sign in with <a href="/_littlebox/owner/login">owner sign-in</a> to manage this app's accounts.</p>;

/** App accounts, invitations and who may join, for the published app. Owners only. */
export function AppUsersView({ deploymentId, visibility, onSetVisibility }: Props) {
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
        <Field label="Who can open the app" help={`${visibilityCopy[visibility].sentence} It applies when you deploy.`}>
          <Select value={visibility} onChange={(event) => onSetVisibility?.(event.target.value === "private" ? "private" : "public")}>
            <option value="public">Anyone with the link</option>
            <option value="private">People with an app account</option>
          </Select>
        </Field>
        {deploymentId
          ? ownerSignedOut ? SIGNED_OUT : <SignupPolicyControl deploymentId={deploymentId} onSignedOut={onSignedOut} onSaved={onChanged} />
          : <p className="access-note">You choose who can create an app account when you deploy. Invitation only is the choice for a private app.</p>}
      </Card>
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
  if (!policy) return <p className="access-note">Loading the signup policy.</p>;

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
  else if (!rows) body = <p className="access-note">Loading invitations.</p>;
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

  return (
    <Card className="access-card" role="group" aria-label="App accounts">
      <header className="access-card-head">
        <h3>App accounts</h3>
        <Select aria-label="Show app accounts" value={filter} onChange={(event) => { setFilter(event.target.value as "" | AppUserState); setCursors([undefined]); }}>
          {stateFilters.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        </Select>
      </header>
      <p className="access-note">People who use this app. They are not owners and cannot open this editor.</p>
      {error && <div className="access-error" role="alert">{error}</div>}
      {!rows ? <p className="access-note">Loading app accounts.</p> : rows.length === 0 ? (
        <p className="access-note">{filter ? `No ${userStateView[filter].label.toLowerCase()} app accounts.` : "No app accounts yet. People appear here once they sign up or you invite them."}</p>
      ) : (
        <div className="access-table-wrap">
          <table className="access-table">
            <thead><tr><th scope="col">Email</th><th scope="col">State</th><th scope="col">Since</th><th scope="col"><span className="sr-only">Actions</span></th></tr></thead>
            <tbody>
              {rows.map((user) => (
                <tr key={user.userId}>
                  <td className="access-email">{user.email}</td>
                  <td><Badge tone={userStateView[user.state].tone} title={userStateView[user.state].sentence}>{userStateView[user.state].label}</Badge>{user.state !== "pending" && !user.verified && <small> Email not verified</small>}</td>
                  <td><small>{formatDate(user.createdAt)}</small></td>
                  <td className="access-row-actions">
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

