import {
  File,
  FileImage,
  HardDriveUpload,
  Plug,
  ShieldCheck,
  Users,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { DatabaseAdminHost, MiniAppRuntimeHost, StoredFile } from "../runtime/host";
import { parseServerActionSource, routeCallers } from "../kernel/actions";
import { Button, Card, EmptyState as UiEmptyState, Field, Select, TabBar } from "./ui";
import { DatabaseStudio } from "./DatabaseStudio";
import { AppUsersView, OpenAccessFields } from "./AppUsersView";
import { IntegrationsView } from "./IntegrationsView";
import { actionSentence, actionTitle, deployReadiness } from "./action-readiness";

export type AuthView = "rules" | "users" | "integrations";

/**
 * The preview keeps these files in this browser only. A Deployment stores its
 * own uploads, and Export takes those, so nothing here reaches either.
 */
export const previewUploadsNote =
  "Files the preview uploads stay in this browser. They do not publish, and Export does not take them. A file every page shows, such as a logo, goes under public/ in Files.";

type Props = {
  view: "database" | "files" | "auth";
  host: MiniAppRuntimeHost;
  onEditSchema: () => void;
  onExportDatabase: () => void;
  onFilesChanged: () => void;
  deployedDatabase?: DatabaseAdminHost;
  databaseTarget?: "local" | "deployed";
  deployedLabel?: string;
  onDatabaseTargetChange?: (target: "local" | "deployed") => void;
  actionsSource?: string;
  onSetAccess?: (name: string, access: "anonymous" | "authenticated") => void;
  visibility?: "public" | "private";
  onSetVisibility?: (visibility: "public" | "private") => void;
  fileAccess?: "app-users";
  onSetFileAccess?: (shared: boolean) => void;
  projectId?: string;
  /** The published app, when there is one. Users and API keys need it. */
  deployment?: { id: string; url: string };
  authView?: AuthView;
  onAuthViewChange?: (view: AuthView) => void;
  /** Changes each time Users should open with the cursor in its invite field. */
  inviteFocus?: number;
  /** The rules, who can open the app or who can read uploads differ from what the live app runs. */
  accessUndeployed?: boolean;
  onDeploy?: () => void;
};

export function CapabilityWorkspace(props: Props) {
  if (props.view === "database") {
    return (
      <DatabaseStudio
        host={props.databaseTarget === "deployed" && props.deployedDatabase ? props.deployedDatabase : props.host}
        onEditSchema={props.onEditSchema}
        onExportDatabase={props.onExportDatabase}
        target={props.databaseTarget}
        deployedLabel={props.deployedLabel}
        onTargetChange={props.deployedDatabase ? props.onDatabaseTargetChange : undefined}
      />
    );
  }
  if (props.view === "files") return <FilesPanel {...props} />;
  return <AuthPanel {...props} />;
}

function FilesPanel({ host, onFilesChanged, deployment }: Props) {
  const [files, setFiles] = useState<StoredFile[]>(host.getFiles());
  const [error, setError] = useState("");
  const [actor, setActor] = useState(host.getPreviewActor());
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => host.subscribeFiles(setFiles), [host]);
  useEffect(() => host.subscribeSession(() => setActor(host.getPreviewActor())), [host]);

  const upload = async (file?: globalThis.File) => {
    if (!file) return;
    setError("");
    try {
      const dataUrl = await readFile(file);
      await host.handle("files.upload", { name: file.name, type: file.type, size: file.size, dataUrl });
      setFiles(host.getFiles());
      onFilesChanged();
    } catch (cause) {
      setError(cause instanceof Error && cause.message === "authentication-required" ? "Choose a preview user before uploading." : cause instanceof Error ? cause.message : String(cause));
    }
  };

  return (
    <section className="cap-workspace" aria-label="Uploads">
      {files.length > 0 && <p className="preview-uploads-context">Test uploads, kept in this browser. The published app keeps its own.{deployment && <> <a href={deployment.url} target="_blank" rel="noreferrer">Open the published app</a></>}</p>}
      <div className="preview-upload-actor"><Field label="Upload as" help={actor === "guest" ? "A signed-out visitor cannot upload. Owner sign-in is separate from the app's users." : "Files you upload belong to this preview user."}><Select value={actor} onChange={(event) => { host.setPreviewActor(event.target.value as "guest" | "alice" | "bob"); setError(""); onFilesChanged(); }}><option value="guest">Signed out</option><option value="alice">Alice</option><option value="bob">Bob</option></Select></Field></div>
      <input ref={inputRef} className="sr-only" type="file" disabled={actor === "guest"} onChange={(event) => { void upload(event.target.files?.[0]); event.target.value = ""; }} />
      {files.length > 0 && (
        <div className="files-summary">
          <span>{files.length} upload{files.length === 1 ? "" : "s"}</span>
          <span>{formatBytes(files.reduce((sum, file) => sum + file.size, 0))}</span>
          <Button variant="primary" className="files-upload" disabled={actor === "guest"} onClick={() => inputRef.current?.click()}><HardDriveUpload size={13} /> Upload file</Button>
        </div>
      )}
      {error && <div className="cap-error" role="alert">{error}</div>}
      <div className="file-grid">
        {files.length === 0 ? <EmptyState onUpload={() => inputRef.current?.click()} disabled={actor === "guest"} reason={actor === "guest" ? "Choose Alice or Bob under Upload as to upload a file." : undefined} text={previewUploadsNote} /> : files.map((file) => (
          <article key={file.id} className="file-card">
            <div className="file-preview">{file.type.startsWith("image/") ? <img src={file.url} alt="" /> : <File size={27} />}</div>
            <div><strong>{file.name}</strong><small>{formatBytes(file.size)} · {file.type || "unknown"}</small></div>
          </article>
        ))}
      </div>
    </section>
  );
}

function AuthPanel(props: Props) {
  const [ownView, setOwnView] = useState<AuthView>("rules");
  const view = props.authView ?? ownView;
  const show = (next: AuthView) => { setOwnView(next); props.onAuthViewChange?.(next); };
  return (
    <section className="cap-workspace" aria-label="Access">
      <TabBar
        label="Access views"
        className="auth-tabs"
        value={view}
        onChange={show}
        items={[
          { id: "rules", label: "Rules", icon: <ShieldCheck size={13} aria-hidden="true" /> },
          { id: "users", label: "Users", icon: <Users size={13} aria-hidden="true" /> },
          { id: "integrations", label: "Integrations", icon: <Plug size={13} aria-hidden="true" /> },
        ]}
      />
      {view === "users" && <AppUsersView projectId={props.projectId} deploymentId={props.deployment?.id} visibility={props.visibility ?? "private"} openAccess={false} inviteFocus={props.inviteFocus} />}
      {view === "integrations" && props.projectId && <IntegrationsView projectId={props.projectId} deploymentId={props.deployment?.id} deploymentUrl={props.deployment?.url} actionsSource={props.actionsSource} />}
      {view === "rules" && <RulesView {...props} />}
    </section>
  );
}

function RulesView({ host, onFilesChanged, actionsSource, onSetAccess, visibility = "private", onSetVisibility, fileAccess, onSetFileAccess, deployment, accessUndeployed }: Props) {
  const [actor, setActor] = useState(host.getPreviewActor());
  const refusal = actionsSource ? deployReadiness({ files: { "actions.json": actionsSource }, manifest: { runtime: { database: { actions: [] } } } }).refusal : null;
  const previewName = actor === "alice" ? "a signed-in person" : actor === "bob" ? "another signed-in person" : "someone who has not signed in";
  const actions = rules(actionsSource);
  return (
    <div className="access-view rules-view">
      {/* The top bar's marker and Deploy button ask for the deploy; this line only states the fact. */}
      {deployment && accessUndeployed && <p className="access-note access-saved" role="status">Saved. It applies to the live app when you deploy.</p>}
      <Card className="access-card" role="group" aria-label="Who can open the app">
        <OpenAccessFields visibility={visibility} onSetVisibility={onSetVisibility} fileAccess={fileAccess} onSetFileAccess={onSetFileAccess} />
      </Card>
      <Card className="access-card" role="group" aria-label="Who can run each action">
        <h3>Who can run each action</h3>
        {visibility === "private" && actions.length > 0 && <p className="access-note">The app is private, so every action needs an app account.</p>}
        {refusal && <p className="cap-error">actions.json does not parse, so no rule can be shown or changed here. {refusal}</p>}
        {actions.length > 0 && <ul className="rule-list">
          {actions.map((action) => {
            const title = actionTitle(action.name);
            // An action whose route is its only door has no access to choose here: its route's callers decide.
            const fixed = Boolean(action.http && routeCallers(action.http, visibility));
            return <li key={action.name}>
              <div className="rule-text">
                <strong>{title}</strong>
                {/* The sentence the Deploy dialog says for this rule on this visibility. */}
                <span>{actionSentence(action, visibility)}</span>
              </div>
              {/* On a private app every action needs an app account, so there is no choice to offer. */}
              {!fixed && visibility === "public" && <Select
                aria-label={`Who can ${title.toLowerCase()}`}
                value={action.access === "anonymous" ? "anonymous" : "authenticated"}
                onChange={(event) => onSetAccess?.(action.name, event.target.value === "anonymous" ? "anonymous" : "authenticated")}
              >
                <option value="anonymous">Anyone</option>
                <option value="authenticated">A signed-in person</option>
              </Select>}
            </li>;
          })}
        </ul>}
        {deployment && !accessUndeployed && <p className="access-note">{appliesOnDeployRules}</p>}
      </Card>
      <Card className="access-card" role="group" aria-label="Preview as">
        <h3>Preview as</h3>
        <Field label="Preview user" help={`The preview runs as ${previewName}. Each app keeps its own sessions. The app decides who a person is, and Konae stores the session.`}><Select value={actor} onChange={(event) => {
          const next = event.target.value as "guest" | "alice" | "bob";
          setActor(next);
          host.setPreviewActor(next);
          onFilesChanged();
        }}><option value="guest">Signed out</option><option value="alice">Signed in as Alice</option><option value="bob">Signed in as Bob</option></Select></Field>
      </Card>
    </div>
  );
}

/** Rules run in the preview at once, and on the live app from the next deploy. */
const appliesOnDeployRules = "A rule runs in the preview at once and on the live app when you deploy.";

function EmptyState({ text, onUpload, disabled, reason }: { text: string; onUpload: () => void; disabled?: boolean; reason?: string }) {
  const reasonId = "upload-disabled-reason";
  return <UiEmptyState className="cap-empty" icon={<FileImage size={26} />} title="No files yet" description={text} action={<div className="cap-empty-action">
    <Button variant="primary" disabled={disabled} aria-describedby={reason ? reasonId : undefined} onClick={onUpload}><HardDriveUpload size={15} /> Upload file</Button>
    {reason && <p id={reasonId} className="cap-empty-reason">{reason}</p>}
  </div>} />;
}

function rules(source: string | undefined) {
  if (!source) return [];
  try {
    return Object.values(parseServerActionSource(source).actions);
  } catch {
    return [];
  }
}

function formatBytes(bytes: number) {
  if (bytes < 1_024) return `${bytes} B`;
  return `${(bytes / 1_024).toFixed(1)} KiB`;
}

function readFile(file: globalThis.File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error);
    reader.onload = () => resolve(String(reader.result));
    reader.readAsDataURL(file);
  });
}
