import { ExternalLink, KeyRound, Rocket } from "lucide-react";
import { Loader2 } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { listSecrets, readContactEmail, readSignupPolicy, type ContactEmailState, type DeploymentReceipt, type SignupMode } from "../deployments/client";
import { missingContactSentence } from "./AppContactEmail";
import { readDomain } from "../deployments/domains";
import type { DeploymentVisibility } from "../deployments/manifest";
import { actionSentence, actionTitle, deployReadiness, ownerFixes, type ReleaseFacts } from "./action-readiness";
import { hintOf, refusalCodes, type RefusalCode } from "../kernel/refusal";
import type { MiniAppProject } from "../lib/types";
import type { OwnerSession } from "./owner-session";
import { defaultSignupMode, firstPublishModes, republishAccessNote, signupModeCopy, visibilityCopy } from "./app-access";
import { Alert, Button, Dialog, Radio, Select } from "./ui";
import "./app-access.css";

type Props = {
  project: MiniAppProject;
  deployment: DeploymentReceipt | null;
  publishing: boolean;
  error: string;
  /** The Kernel's refusal code for `error`, when it refused the Capsule (ADR-0004). */
  errorCode?: string;
  /** Signed out means deploying cannot work, so the dialog offers sign-in instead. */
  session: OwnerSession;
  signInHref: string;
  onClose: () => void;
  /** The app as it is published now, or null before the first publish. */
  current?: DeploymentReceipt | null;
  /** `signupMode` is set only on a first publish. */
  onDeploy: (visibility: DeploymentVisibility, signupMode?: SignupMode) => void;
  onEditAuth: () => void;
  onOpenUsers?: () => void;
  /** Opens Access > Integrations, where the owner sets a signing secret. */
  onOpenIntegrations?: () => void;
  onOpenAppSettings?: () => void;
  /** Opens actions.json in Files, where an edit clears the refusal. */
  onOpenActions?: () => void;
  /** True when the editor holds exactly what the published app runs, false when it differs, null when the release does not say. */
  unchanged?: boolean | null;
};

function republishSentence(unchanged: boolean | null) {
  if (unchanged === true) return "Nothing changed since the last deploy, so this publishes the same app again. Its accounts and rows stay.";
  if (unchanged === false) return "Your edits replace what the published app runs. Its accounts and rows stay.";
  return "This publishes what the editor holds now. The app keeps its accounts and rows.";
}

type PolicyRead = { status: "loading" } | { status: "ready"; mode: SignupMode } | { status: "unknown"; reason: string };

/** Reads who may create an app account on a published app, for display only. */
function usePublishedPolicy(deploymentId: string | undefined) {
  const [policy, setPolicy] = useState<PolicyRead>({ status: "loading" });
  useEffect(() => {
    if (!deploymentId) return;
    let live = true;
    setPolicy({ status: "loading" });
    readSignupPolicy(deploymentId).then((value) => { if (live) setPolicy({ status: "ready", mode: value.mode }); }, (cause) => {
      if (live) setPolicy({ status: "unknown", reason: cause instanceof Error ? cause.message : String(cause) });
    });
    return () => { live = false; };
  }, [deploymentId]);
  return policy;
}

/**
 * What release_plan reads that the editor can read too: the project's secret
 * names, uses and svix-key flags (never values) and, once the app is published, the installation's
 * hosts. A read that fails leaves its part out, and the check stays silent on it.
 */
function useReleaseFacts(projectId: string, deploymentId: string | undefined, signedIn: boolean): ReleaseFacts {
  const [secrets, setSecrets] = useState<ReleaseFacts["secrets"]>();
  const [rules, setRules] = useState<ReleaseFacts["rules"]>();
  useEffect(() => {
    if (!signedIn) return;
    let live = true;
    listSecrets(projectId).then((answer) => {
      if (live) setSecrets(answer.secrets);
    }, () => undefined);
    return () => { live = false; };
  }, [projectId, signedIn]);
  useEffect(() => {
    if (!signedIn || !deploymentId) return;
    let live = true;
    readDomain(deploymentId).then((view) => { if (live && view.rules) setRules(view.rules); }, () => undefined);
    return () => { live = false; };
  }, [deploymentId, signedIn]);
  return { secrets, rules };
}

/** The owner clears this refusal by setting a secret, which Access > Integrations does. */
function setsSecret(code: string | undefined): boolean {
  return code !== undefined && refusalCodes().includes(code as RefusalCode) && hintOf(code as RefusalCode).tool === "secret_set";
}

export function DeployDialog({ project, deployment, publishing, error, errorCode, session, signInHref, onClose, onDeploy, onEditAuth, current = null, onOpenUsers, onOpenIntegrations, onOpenAppSettings, onOpenActions, unchanged = null }: Props) {
  const republish = Boolean(current);
  const [chosenVisibility, setVisibility] = useState<DeploymentVisibility>(project.manifest.visibility);
  // A republish keeps who can join, set in Access > Users; only a first publish chooses here.
  const visibility = republish ? project.manifest.visibility : chosenVisibility;
  const [chosenMode, setChosenMode] = useState<SignupMode | null>(null);
  const signupMode = chosenMode ?? defaultSignupMode(visibility);
  const publishedPolicy = usePublishedPolicy(republish && !deployment ? current?.id : undefined);
  const signedOut = session.state === "signed-out";
  const facts = useReleaseFacts(project.id, current?.id, session.state === "signed-in");
  const { refusal, ownerActs, actions } = deployReadiness(project, facts);
  const errorRef = useRef<HTMLDivElement>(null);
  const signupPolicyId = useId();
  const address = current?.url.replace(/^https?:\/\//, "").replace(/\/$/, "");

  // A failed deploy says why where focus is, so a screen reader hears it and
  // the keyboard stays inside the dialog.
  useEffect(() => {
    if (error && !publishing) errorRef.current?.focus();
  }, [error, publishing]);

  const deployLabel = republish ? "Deploy changes" : `Deploy ${visibility}`;

  return (
    <Dialog
      title={deployment ? `${project.name} is published` : `Deploy ${project.name}`}
      description={deployment ? undefined : republish ? republishSentence(unchanged) : "Deploy this app so people can open it at its own address."}
      onClose={onClose}
      className={`deploy-dialog${refusal ? " is-refused" : ""}`}
      footer={deployment ? undefined : <>
        {signedOut && <p className="publish-footnote"><KeyRound size={13} aria-hidden="true" /> Sign in as the owner to deploy. You come back to this app.</p>}
        <Button onClick={onClose}>Cancel</Button>
        {signedOut ? (
          <a className="ui-button ui-button-md ui-button-primary deploy-confirm" href={signInHref}><KeyRound size={14} /> Sign in to deploy</a>
        ) : (
          // A refusal disables the button outright. While publishing it stays
          // enabled and says so: disabling the focused button would drop focus
          // to the page body, and Escape and Tab would stop reaching the dialog.
          <Button
            variant="primary"
            className="deploy-confirm"
            disabled={refusal !== null}
            aria-disabled={publishing || undefined}
            aria-busy={publishing || undefined}
            onClick={() => { if (!publishing && !refusal) onDeploy(visibility, republish ? undefined : signupMode); }}
          >
            {publishing ? <><Loader2 className="spin" size={14} /> Deploying…</> : <><Rocket size={14} /> {deployLabel}</>}
          </Button>
        )}
      </>}
    >
      {deployment ? (
        <PublishedState deployment={deployment} onClose={onClose} onOpenUsers={onOpenUsers} onOpenAppSettings={onOpenAppSettings} />
      ) : (
        <div className="ui-dialog-body publish-body">
          {refusal ? (
            <Alert tone="danger" title="Deploy is refused" action={ownerActs
              ? onOpenIntegrations && <Button size="sm" onClick={onOpenIntegrations}>Open Access &gt; Integrations</Button>
              : onOpenActions && <Button size="sm" onClick={onOpenActions}>Open actions.json</Button>}>
              {refusal}
            </Alert>
          ) : (
            <>
              <dl className="publish-facts">
                <div>
                  <dt>Address</dt>
                  <dd>{current && address
                    ? <a className="publish-address" href={current.url} target="_blank" rel="noreferrer">{address}<ExternalLink size={12} aria-hidden="true" /></a>
                    : "Shown after the first deploy."}</dd>
                </div>
                <div>
                  <dt>Who can open it</dt>
                  <dd>
                    {republish
                      ? <>{visibilityCopy[visibility].label}. {visibilityCopy[visibility].sentence}</>
                      : (
                        <div className="publish-visibility" role="radiogroup" aria-label="Deployment visibility">
                          <Radio name="deployment-visibility" value="public" checked={visibility === "public"} onChange={() => setVisibility("public")} label={<><strong>Public</strong> <span>Anyone with the link</span></>} />
                          <Radio name="deployment-visibility" value="private" checked={visibility === "private"} onChange={() => setVisibility("private")} label={<><strong>Private</strong> <span>Only people with an app account</span></>} />
                        </div>
                      )}
                  </dd>
                </div>
                <div>
                  <dt>{republish ? "Who can create an app account" : <label htmlFor={signupPolicyId}>Who can create an app account</label>}</dt>
                  <dd>
                    {republish
                      ? publishedPolicy.status === "ready" ? <>{signupModeCopy[publishedPolicy.mode].label}. {signupModeCopy[publishedPolicy.mode].sentence}</>
                        : publishedPolicy.status === "loading" ? <span className="publish-reading">Reading…</span>
                          : signedOut ? "Sign in as the owner to read it." : `Could not read it: ${publishedPolicy.reason}`
                      : (
                        <>
                          <Select id={signupPolicyId} aria-describedby={`${signupPolicyId}-help`} value={signupMode} onChange={(event) => setChosenMode(event.target.value as SignupMode)}>
                            {firstPublishModes.map((mode) => <option key={mode} value={mode}>{signupModeCopy[mode].label}</option>)}
                          </Select>
                          <p id={`${signupPolicyId}-help`} className="publish-note">{signupModeCopy[signupMode].sentence}</p>
                        </>
                      )}
                  </dd>
                </div>
              </dl>
              {republish && current && current.manifest.visibility !== visibility && <p className="publish-note">{republishAccessNote({ visibility: current.manifest.visibility, ...(publishedPolicy.status === "ready" ? { mode: publishedPolicy.mode } : {}) }, visibility)}</p>}
              {actions.length > 0 && (
                <section className="publish-can-do" aria-label="What the app lets people do">
                  <h3>What people can do</h3>
                  <ul>
                    {actions.map((action) => (
                      <li key={action.name}><strong>{actionTitle(action.name)}</strong> <span>{actionSentence(action, visibility)}</span></li>
                    ))}
                  </ul>
                </section>
              )}
              <button type="button" className="link-button" onClick={onEditAuth}>Change access</button>
            </>
          )}

          {error && (
            <div className="publish-failure" tabIndex={-1} ref={errorRef}>
              <Alert tone="danger" title="Deployment failed" action={onOpenIntegrations && ownerFixes(errorCode) && setsSecret(errorCode) ? <Button size="sm" onClick={onOpenIntegrations}>Set it in Access &gt; Integrations</Button> : signedOut ? <a className="ui-button ui-button-sm ui-button-secondary" href={signInHref}>Sign in to deploy</a> : undefined}>{error}</Alert>
            </div>
          )}
        </div>
      )}
    </Dialog>
  );
}

function PublishedState({ deployment, onClose, onOpenUsers, onOpenAppSettings }: { deployment: DeploymentReceipt; onClose: () => void; onOpenUsers?: () => void; onOpenAppSettings?: () => void }) {
  const isPrivate = deployment.manifest.visibility === "private";
  const policy = usePublishedPolicy(deployment.id);
  const [contact, setContact] = useState<ContactEmailState | null>(null);
  useEffect(() => {
    let current = true;
    readContactEmail(deployment.id).then((value) => { if (current) setContact(value); }, () => {});
    return () => { current = false; };
  }, [deployment.id]);
  const missingContact = missingContactSentence(contact);
  const openRef = useRef<HTMLAnchorElement>(null);
  // The Deploy button that had focus is gone now; the way to the app takes it.
  useEffect(() => { openRef.current?.focus(); }, []);
  return <div className="ui-dialog-body deploy-receipt">
    <a className="published-url" href={deployment.url} target="_blank" rel="noreferrer"><span>{deployment.url}</span><ExternalLink size={14} aria-hidden="true" /></a>
    <p className="receipt-policy">{isPrivate ? "Private. Visitors sign in to an app account before the app loads." : "Public. Anyone with the link can open it."}{policy.status === "ready" && ` New app accounts: ${signupModeCopy[policy.mode].label}.`}</p>
    {deployment.accessCode && <div className="receipt-code"><small>Private access code</small><code>{deployment.accessCode}</code><p>Keep it private. Anyone with this code can open this deployment.</p></div>}
    {missingContact && <p className="deploy-contact-missing" role="status">{missingContact}{onOpenAppSettings && <> <button type="button" className="link-button" onClick={onOpenAppSettings}>Set a contact email</button></>}</p>}
    <div className="receipt-actions">
      <a ref={openRef} className="ui-button ui-button-md ui-button-primary published-open" href={deployment.url} target="_blank" rel="noreferrer"><ExternalLink size={14} aria-hidden="true" /> Open the app</a>
      {policy.status === "ready" && policy.mode === "invitation" && onOpenUsers && <Button onClick={onOpenUsers}>Invite people</Button>}
      <Button onClick={onClose}>Done</Button>
    </div>
  </div>;
}
