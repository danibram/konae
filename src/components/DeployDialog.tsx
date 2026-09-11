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
import type { DeploymentReceipt } from "../deployments/client";
import type { DeploymentVisibility, RuntimeActionAccess } from "../deployments/manifest";
import { parseServerActionSource } from "../kernel/actions";
import { LIMIT_REFUSE_SENTENCE } from "../lib/query-bound";
import type { MiniAppProject } from "../lib/types";
import type { OwnerSession } from "./owner-session";
import { Button, Dialog } from "./ui";

type Props = {
  project: MiniAppProject;
  deployment: DeploymentReceipt | null;
  publishing: boolean;
  error: string;
  /** Signed out means deploying cannot work, so the dialog offers sign-in instead. */
  session: OwnerSession;
  signInHref: string;
  onClose: () => void;
  onDeploy: (visibility: DeploymentVisibility) => void;
  onEditAuth: () => void;
};

export function DeployDialog({ project, deployment, publishing, error, session, signInHref, onClose, onDeploy, onEditAuth }: Props) {
  const [visibility, setVisibility] = useState<DeploymentVisibility>(project.manifest.visibility);
  const actions = describedActions(project);
  const errorRef = useRef<HTMLDivElement>(null);
  const signedOut = session.state === "signed-out";
  const listCanLeave = actions.every((action) => action.kind !== "query" || /\bLIMIT\s+\d+/i.test(action.statement ?? ""));

  // A failed deploy says why where focus is, so a screen reader hears it and
  // the keyboard stays inside the dialog.
  useEffect(() => {
    if (error && !publishing) errorRef.current?.focus();
  }, [error, publishing]);

  return (
    <Dialog title="Deploy application" showTitle={false} onClose={onClose} className={`publish-dialog${signedOut ? " is-signed-out" : ""}${listCanLeave ? "" : " is-refused-leave"}`}>
        {deployment ? (
          <PublishedState deployment={deployment} onClose={onClose} />
        ) : (
          <>
            <div className="publish-scroll">
            <header className="publish-heading">
              <h2>Deploy {project.name}</h2>
              <p>Accounts, rows, and rules leave with this app. Export takes the same folder to another machine.</p>
              <p className={`publish-limit${listCanLeave ? "" : " is-refused"}`}>{listCanLeave ? "This list stops at 100 rows. It can leave." : LIMIT_REFUSE_SENTENCE}</p>
            </header>

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
                <span><strong>Private</strong><small>A signed-in person</small></span>
                <i aria-hidden="true">{visibility === "private" && <Check size={12} />}</i>
              </label>
            </div>

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
                <li key={action.name}><strong>{actionTitle(action.name)}</strong><span>{actionSentence(action)}</span></li>
              ))}
            </ul>
            <button type="button" className="deploy-auth-link" onClick={onEditAuth}>Change this in Auth</button>

            <div className={`visibility-explainer ${visibility}`}>
              {visibility === "private" ? <LockKeyhole size={15} /> : <Globe2 size={15} />}
              <p>{visibility === "private"
                ? "Visitors must sign in before the app loads. Who can create an account is set in Auth > Signup."
                : actions.some((action) => action.access === "anonymous")
                  ? "The app is public. Anyone can run the actions marked Anyone. The rest ask for a sign-in."
                  : "The app is public. Every action asks for a sign-in."}</p>
            </div>

            {error && (
              <div className="publish-error" role="alert" tabIndex={-1} ref={errorRef}>
                <strong>Deployment failed</strong>
                <span>{error}</span>
                {signedOut && <a href={signInHref}>Sign in to deploy</a>}
              </div>
            )}

            </div>
            <footer className="publish-actions">
              {signedOut ? (
                <div className="publish-signin"><KeyRound size={13} /><span>Sign in as the owner to deploy. You come back to this app.</span></div>
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
                  onClick={() => { if (!publishing && listCanLeave) onDeploy(visibility); }}
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

function describedActions(project: MiniAppProject): Array<{ name: string; kind: string; access: RuntimeActionAccess; statement?: string }> {
  try {
    return Object.values(parseServerActionSource(project.files["actions.json"] ?? "").actions);
  } catch {
    return project.manifest.runtime.database.actions;
  }
}

function actionTitle(name: string): string {
  const verb = name.split(".").pop() ?? name;
  if (verb === "list") return "List the rows";
  if (verb === "create") return "Add a row";
  if (verb === "toggle") return "Mark a row done";
  return verb.replaceAll("_", " ");
}

function actionSentence(action: { name: string; kind: string; access: string; statement?: string }): string {
  const who = action.access === "anonymous" ? "Anyone" : "A signed-in person";
  const limit = action.statement?.match(/\bLIMIT\s+(\d+)/i);
  if (action.kind === "query" && limit) return `${who} can run it. The list stops at ${limit[1]} rows.`;
  if (action.kind === "query") return `${who} can run it. This list has no limit.`;
  return `${who} can run it. It can leave.`;
}

function PublishedState({ deployment, onClose }: { deployment: DeploymentReceipt; onClose: () => void }) {
  const isPrivate = deployment.manifest.visibility === "private";
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
      <div><strong>{isPrivate ? "A signed-in person" : "Anyone with the link"}</strong><p>{isPrivate ? "Who can create an account is set in Auth." : "Visitors can open the app, but only declared actions can reach data."}</p></div>
    </div>
    {deployment.accessCode && <div className="published-access-code"><small>PRIVATE ACCESS CODE</small><code>{deployment.accessCode}</code><p>Keep it private. Anyone with this code can open this deployment.</p></div>}
    <p className="deploy-note">Served by the local Littlebox Kernel. The same deployment contract can be backed by any infrastructure adapter.</p>
    <Button variant="primary" className="published-done" onClick={onClose}>Back to workshop</Button>
  </div>;
}
