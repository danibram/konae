import {
  Check,
  ExternalLink,
  Globe2,
  KeyRound,
  LockKeyhole,
  Rocket,
  ShieldCheck,
} from "lucide-react";
import { Loader2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { listSecrets, readSignupPolicy, type DeploymentReceipt, type SignupMode } from "../deployments/client";
import { readDomain } from "../deployments/domains";
import type { DeploymentVisibility } from "../deployments/manifest";
import { actionSentence, deployReadiness, ownerFixes, type ReleaseFacts } from "./action-readiness";
import type { MiniAppProject } from "../lib/types";
import type { OwnerSession } from "./owner-session";
import { defaultSignupMode, firstPublishModes, republishAccessNote, signupModeCopy, visibilityCopy } from "./app-access";
import { Button, Dialog, Field, Select } from "./ui";
import "./app-access.css";

type Props = {
  project: MiniAppProject;
  deployment: DeploymentReceipt | null;
  publishing: boolean;
  error: string;
  /** The Kernel's refusal code for `error`, when it refused the Capsule (ADR-0020). */
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
  /** Opens Auth > Integrations, where the owner sets a signing secret. */
  onOpenIntegrations?: () => void;
  onOpenAppSettings?: () => void;
};

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

export function DeployDialog({ project, deployment, publishing, error, errorCode, session, signInHref, onClose, onDeploy, onEditAuth, current = null, onOpenUsers, onOpenIntegrations, onOpenAppSettings }: Props) {
  const republish = Boolean(current);
  const [chosenVisibility, setVisibility] = useState<DeploymentVisibility>(project.manifest.visibility);
  // A republish keeps the access set in Auth > Users; only a first publish chooses here.
  const visibility = republish ? project.manifest.visibility : chosenVisibility;
  const [chosenMode, setChosenMode] = useState<SignupMode | null>(null);
  const signupMode = chosenMode ?? defaultSignupMode(visibility);
  const publishedPolicy = usePublishedPolicy(republish && !deployment ? current?.id : undefined);
  const signedOut = session.state === "signed-out";
  const facts = useReleaseFacts(project.id, current?.id, session.state === "signed-in");
  const { refusal, ownerActs, actions } = deployReadiness(project, facts);
  const errorRef = useRef<HTMLDivElement>(null);
  const listCanLeave = refusal === null;

  // A failed deploy says why where focus is, so a screen reader hears it and
  // the keyboard stays inside the dialog.
  useEffect(() => {
    if (error && !publishing) errorRef.current?.focus();
  }, [error, publishing]);

  return (
    <Dialog title="Deploy application" showTitle={false} onClose={onClose} className={`publish-dialog${signedOut ? " is-signed-out" : ""}${listCanLeave ? "" : " is-refused-leave"}`}>
        {deployment ? (
          <PublishedState deployment={deployment} onClose={onClose} onOpenUsers={onOpenUsers} onOpenAppSettings={onOpenAppSettings} />
        ) : (
          <>
            <div className="publish-scroll">
            <header className="publish-heading">
              <h2>Deploy {project.name}</h2>
              <p>Accounts, rows, and rules leave with this app. Export takes the same folder to another machine.</p>
              <p className={`publish-limit${listCanLeave ? "" : " is-refused"}`}>{refusal ?? "Every query names its LIMIT. It can leave."}</p>
            </header>

            {republish && current ? (
              <section className="publish-access" aria-label="Access after this publish">
                <dl>
                  <div><dt>Who can open the app</dt><dd>{visibilityCopy[visibility].label}. {visibilityCopy[visibility].sentence}</dd></div>
                  <div><dt>Who can create an app account</dt><dd>{publishedPolicy.status === "ready" ? <>{signupModeCopy[publishedPolicy.mode].label}. {signupModeCopy[publishedPolicy.mode].sentence}</> : publishedPolicy.status === "loading" ? "Reading the published policy." : signedOut ? "Sign in as the owner to read it." : `Could not read it: ${publishedPolicy.reason}`}</dd></div>
                </dl>
                <p>{republishAccessNote({ visibility: current.manifest.visibility, ...(publishedPolicy.status === "ready" ? { mode: publishedPolicy.mode } : {}) }, visibility)}</p>
                {onOpenUsers && <button type="button" className="deploy-auth-link" onClick={onOpenUsers}>Open Auth &gt; Users</button>}
                {onOpenAppSettings && <button type="button" className="deploy-auth-link" onClick={onOpenAppSettings}>Manage custom domain</button>}
              </section>
            ) : (
              <>
                <div className="visibility-options" role="radiogroup" aria-label="Deployment visibility">
                <label className={visibility === "public" ? "selected" : ""}>
                  <input className="sr-only" type="radio" name="deployment-visibility" value="public" checked={visibility === "public"} onChange={() => setVisibility("public")} />
                  <span className="visibility-icon public"><Globe2 size={20} /></span>
                  <span><strong>Public</strong><small>Anyone can open the app</small></span>
                  <i aria-hidden="true">{visibility === "public" && <Check size={12} />}</i>
                </label>
                <label className={visibility === "private" ? "selected" : ""}>
                  <input className="sr-only" type="radio" name="deployment-visibility" value="private" checked={visibility === "private"} onChange={() => setVisibility("private")} />
                  <span className="visibility-icon private"><LockKeyhole size={20} /></span>
                  <span><strong>Private</strong><small>People with an app account</small></span>
                  <i aria-hidden="true">{visibility === "private" && <Check size={12} />}</i>
                </label>
              </div>

                <div className="publish-signup">
                  <Field label="Who can create an app account" help={`${signupModeCopy[signupMode].sentence} You can change it later in Auth > Users.`}>
                    <Select value={signupMode} onChange={(event) => setChosenMode(event.target.value as SignupMode)}>
                      {firstPublishModes.map((mode) => <option key={mode} value={mode}>{signupModeCopy[mode].label}</option>)}
                    </Select>
                  </Field>
                </div>
              </>
            )}

            <section className={`deployment-boundary ${listCanLeave ? "" : "is-refused"}`}>
              <header><ShieldCheck size={15} /><strong>What leaves</strong></header>
              <div className="boundary-columns">
                <article>
                  <strong>In the published app</strong>
                  <ul>
                    <li>The accounts</li>
                    <li>The rows</li>
                    <li>The rules</li>
                  </ul>
                </article>
                <article>
                  <strong>In the folder you keep</strong>
                  <ul>
                    <li>The same accounts</li>
                    <li>The same rows</li>
                    <li>The same rules</li>
                  </ul>
                </article>
              </div>
            </section>

            <ul className="deploy-made">
              {actions.map((action) => (
                <li key={action.name}><strong>{actionTitle(action.name)}</strong><span>{actionSentence(action, visibility)}</span></li>
              ))}
            </ul>
            <button type="button" className="deploy-auth-link" onClick={onEditAuth}>Change this in Auth</button>

            <div className={`visibility-explainer ${visibility}`}>
              {visibility === "private" ? <LockKeyhole size={15} /> : <Globe2 size={15} />}
              <p>{visibility === "private"
                ? "Visitors must sign in to an app account before the app loads. This is not owner sign-in; owners use the editor."
                : actions.some((action) => action.access === "anonymous")
                  ? "The app is public. Anyone can run the actions marked Anyone. The rest ask for a sign-in."
                  : "The app is public. Every action asks for a sign-in."}</p>
            </div>

            {error && (
              <div className="publish-error" role="alert" tabIndex={-1} ref={errorRef}>
                <strong>Deployment failed</strong>
                <span>{error}</span>
                {signedOut && <a href={signInHref}>Sign in to deploy</a>}
                {onOpenIntegrations && ownerFixes(errorCode) && <button type="button" className="deploy-auth-link" onClick={onOpenIntegrations}>Set it in Auth &gt; Integrations</button>}
              </div>
            )}

            </div>
            <footer className="publish-actions">
              {signedOut ? (
                <div className="publish-signin"><KeyRound size={13} /><span>Sign in as the owner to deploy. You come back to this app.</span></div>
              ) : refusal ? (
                <div className="publish-refusal" role="status">
                  <KeyRound size={13} />
                  <span>{ownerActs ? "Deploy is refused until the owner sets the secret named above." : "Deploy is refused until actions.json changes as the line above says."}</span>
                  {ownerActs && onOpenIntegrations && <button type="button" className="deploy-auth-link" onClick={onOpenIntegrations}>Open Auth &gt; Integrations</button>}
                </div>
              ) : (
                <div><KeyRound size={13} /><span>Accounts, rows and rules stay with this app.</span></div>
              )}
              <Button className="cancel-publish" onClick={onClose}>Cancel</Button>
              {signedOut ? (
                <a
                  className={`ui-button ui-button-md confirm-publish ${listCanLeave ? "ui-button-primary" : "ui-button-secondary"}`}
                  href={signInHref}
                >
                  <KeyRound size={14} /> Sign in to deploy
                </a>
              ) : (
                // Never disabled while publishing. A disabled button drops focus
                // to the page body, and Escape and Tab stop reaching the dialog.
                // When the check refuses, keep the control secondary so the refuse
                // panel owns the footer; do not leave while the list has no LIMIT.
                <Button
                  variant={listCanLeave ? "primary" : "secondary"}
                  className="confirm-publish"
                  aria-disabled={publishing || !listCanLeave || undefined}
                  aria-busy={publishing || undefined}
                  onClick={() => { if (!publishing && listCanLeave) onDeploy(visibility, republish ? undefined : signupMode); }}
                >
                  {publishing ? <><Loader2 className="spin" size={14} /> Publishing…</> : <><Rocket size={14} /> Deploy {visibility}</>}
                </Button>
              )}
            </footer>
          </>
        )}
    </Dialog>
  );
}

function actionTitle(name: string): string {
  const verb = name.split(".").pop() ?? name;
  if (verb === "list") return "List the rows";
  if (verb === "create") return "Add a row";
  if (verb === "toggle") return "Mark a row done";
  return verb.replaceAll("_", " ");
}

function PublishedState({ deployment, onClose, onOpenUsers, onOpenAppSettings }: { deployment: DeploymentReceipt; onClose: () => void; onOpenUsers?: () => void; onOpenAppSettings?: () => void }) {
  const isPrivate = deployment.manifest.visibility === "private";
  const policy = usePublishedPolicy(deployment.id);
  const urlRef = useRef<HTMLAnchorElement>(null);
  // The Deploy button that had focus is gone now; the new address takes it.
  useEffect(() => { urlRef.current?.focus(); }, []);
  return <div className="published-state">
    <span className={`published-orbit ${isPrivate ? "private" : "public"}`}>{isPrivate ? <LockKeyhole size={27} /> : <Globe2 size={27} />}</span>
    <p className="publish-kicker"><Check size={12} /> DEPLOYMENT READY</p>
    <h2>Published {isPrivate ? "privately" : "publicly"}</h2>
    <a ref={urlRef} className="published-url" href={deployment.url} target="_blank" rel="noreferrer"><span>{deployment.url}</span><ExternalLink size={14} /></a>
    <div className="published-policy">
      <ShieldCheck size={17} />
      <div><strong>{isPrivate ? "People with an app account" : "Anyone with the link"}</strong><p>{isPrivate ? "Visitors sign in to an app account before the app loads." : "Visitors can open the app, but only declared actions can reach data."}{policy.status === "ready" && ` New app accounts: ${signupModeCopy[policy.mode].label}.`}</p></div>
    </div>
    {deployment.accessCode && <div className="published-access-code"><small>PRIVATE ACCESS CODE</small><code>{deployment.accessCode}</code><p>Keep it private. Anyone with this code can open this deployment.</p></div>}
    <p className="deploy-note">Accounts, rows and rules are published with this app. Export takes them to another machine.</p>
    {policy.status === "ready" && policy.mode === "invitation" && onOpenUsers && <Button className="published-invite" onClick={onOpenUsers}>Invite people</Button>}
    {onOpenAppSettings && <Button onClick={onOpenAppSettings}>Connect a custom domain</Button>}
    <Button variant="primary" className="published-done" onClick={onClose}>Back to workshop</Button>
  </div>;
}
