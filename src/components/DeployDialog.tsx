import {
  Check,
  Code2,
  Database,
  ExternalLink,
  Globe2,
  KeyRound,
  LockKeyhole,
  Rocket,
  ShieldCheck,
  X,
} from "lucide-react";
import { useState } from "react";
import type { DeploymentReceipt } from "../deployments/client";
import type { DeploymentVisibility } from "../deployments/manifest";
import type { MiniAppProject } from "../lib/types";
import { useModalDialog } from "./useModalDialog";

type Props = {
  project: MiniAppProject;
  deployment: DeploymentReceipt | null;
  publishing: boolean;
  error: string;
  onClose: () => void;
  onDeploy: (visibility: DeploymentVisibility) => void;
};

export function DeployDialog({ project, deployment, publishing, error, onClose, onDeploy }: Props) {
  const [visibility, setVisibility] = useState<DeploymentVisibility>(project.manifest.visibility);
  const actions = project.manifest.runtime.database.actions;
  // The dialog closes by unmounting, so it is open for as long as it exists.
  const dialog = useModalDialog({ onClose });

  return (
    <div className="modal-backdrop deploy-backdrop" role="presentation" onClick={onClose}>
      <section
        className="publish-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Deploy application"
        ref={dialog.ref}
        onKeyDown={dialog.onKeyDown}
        onClick={(event) => event.stopPropagation()}
      >
        <button className="publish-close" aria-label="Close deploy dialog" onClick={onClose}><X size={16} /></button>
        {deployment ? (
          <PublishedState deployment={deployment} onClose={onClose} />
        ) : (
          <>
            <header className="publish-heading">
              <span className="publish-kicker"><Rocket size={12} /> DEPLOYMENT / NEW RELEASE</span>
              <h2>Put <em>{project.name}</em><br />on the internet.</h2>
              <p>The code is publishable. The database stays behind Littlebox.</p>
            </header>

            <div className="visibility-options" role="radiogroup" aria-label="Deployment visibility">
              <button role="radio" aria-checked={visibility === "public"} className={visibility === "public" ? "selected" : ""} onClick={() => setVisibility("public")}>
                <span className="visibility-icon public"><Globe2 size={20} /></span>
                <span><strong>Public</strong><small>Anyone can open the app</small></span>
                <i>{visibility === "public" && <Check size={12} />}</i>
              </button>
              <button role="radio" aria-checked={visibility === "private"} className={visibility === "private" ? "selected" : ""} onClick={() => setVisibility("private")}>
                <span className="visibility-icon private"><LockKeyhole size={20} /></span>
                <span><strong>Private</strong><small>Login required</small></span>
                <i>{visibility === "private" && <Check size={12} />}</i>
              </button>
            </div>

            <section className="deployment-boundary">
              <header><ShieldCheck size={15} /><strong>Data boundary</strong><span>enforced by Littlebox Kernel</span></header>
              <div className="boundary-columns">
                <article>
                  <p><Code2 size={13} /> DEPLOYED APP</p>
                  <strong>{actions.length} named actions</strong>
                  <ul>
                    <li>No raw SQL</li>
                    <li>No database credentials</li>
                    <li>Files follow app visibility</li>
                  </ul>
                </article>
                <article>
                  <p><Database size={13} /> OWNER EDITOR</p>
                  <strong>Administrative access</strong>
                  <ul>
                    <li>SQL Studio &amp; schema</li>
                    <li>Backups &amp; export</li>
                    <li>Owner-only destructive writes</li>
                  </ul>
                </article>
              </div>
              <div className="action-strip">
                {actions.map((action) => <span key={action.name}><i className={action.kind} />{action.name}<small>{visibility === "private" ? "authenticated" : action.access}</small></span>)}
              </div>
            </section>

            <div className={`visibility-explainer ${visibility}`}>
              {visibility === "private" ? <LockKeyhole size={15} /> : <Globe2 size={15} />}
              <p>{visibility === "private"
                ? "Visitors must create an account or sign in before opening this app. Each action still enforces its data policy."
                : "The app shell is public. Each action still applies its own data policy."}</p>
            </div>

            {error && <div className="publish-error" role="alert"><strong>Deployment failed</strong><span>{error}</span></div>}

            <footer className="publish-actions">
              <div><KeyRound size={13} /><span>Manifest v1 · database protected</span></div>
              <button className="cancel-publish" onClick={onClose}>Cancel</button>
              <button className="confirm-publish" disabled={publishing} onClick={() => onDeploy(visibility)}>
                {publishing ? "Publishing…" : <><Rocket size={14} /> Deploy {visibility}</>}
              </button>
            </footer>
          </>
        )}
      </section>
    </div>
  );
}

function PublishedState({ deployment, onClose }: { deployment: DeploymentReceipt; onClose: () => void }) {
  const isPrivate = deployment.manifest.visibility === "private";
  return <div className="published-state">
    <span className={`published-orbit ${isPrivate ? "private" : "public"}`}>{isPrivate ? <LockKeyhole size={27} /> : <Globe2 size={27} />}</span>
    <p className="publish-kicker"><Check size={12} /> DEPLOYMENT READY</p>
    <h2>Published<br /><em>{isPrivate ? "privately." : "publicly."}</em></h2>
    <a className="published-url" href={deployment.url} target="_blank" rel="noreferrer"><span>{deployment.url}</span><ExternalLink size={14} /></a>
    <div className="published-policy">
      <ShieldCheck size={17} />
      <div><strong>{isPrivate ? "Deployment grant required" : "Public shell, protected data"}</strong><p>{isPrivate ? "Unauthorised visitors are stopped before the app loads." : "Visitors can open the app, but only declared actions can reach data."}</p></div>
    </div>
    {deployment.accessCode && <div className="published-access-code"><small>PRIVATE ACCESS CODE</small><code>{deployment.accessCode}</code><p>Keep it private. Anyone with this code can create a grant for this deployment.</p></div>}
    <p className="deploy-note">Served by the local Littlebox Kernel. The same deployment contract can be backed by any infrastructure adapter.</p>
    <button className="published-done" onClick={onClose}>Back to workshop</button>
  </div>;
}
