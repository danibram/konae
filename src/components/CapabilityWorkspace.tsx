import {
  File,
  FileImage,
  HardDriveUpload,
  KeyRound,
  Plug,
  ShieldCheck,
  UserRound,
  Users,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { DatabaseAdminHost, MiniAppRuntimeHost, StoredFile } from "../runtime/host";
import type { DrizzleDatabaseModel } from "../lib/types";
import { parseServerActionSource, routeCallers } from "../kernel/actions";
import { Button, EmptyState as UiEmptyState, Field, Select } from "./ui";
import { DatabaseStudio } from "./DatabaseStudio";
import { AppUsersView } from "./AppUsersView";
import { IntegrationsView } from "./IntegrationsView";
import { authHeading } from "./auth-heading";
import { actionSentence, deployReadiness } from "./action-readiness";

export type AuthView = "rules" | "users" | "integrations";

/**
 * The preview keeps these files in this browser only. A Deployment stores its
 * own uploads, and Export takes those, so nothing here reaches either.
 */
export const previewUploadsNote =
  "Files the preview app uploads, kept in this browser only. They do not publish and Export does not take them. A file every page shows, such as a logo, goes under public/ in Files.";

type Props = {
  view: "database" | "files" | "auth";
  host: MiniAppRuntimeHost;
  databaseModel: DrizzleDatabaseModel;
  onEditSchema: () => void;
  onDatabaseModelAccepted: (source: string, revision: string) => void | Promise<void>;
  onDatabaseModelRestored: (source: string, revision: string) => void | Promise<void>;
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
  projectId?: string;
  /** The published app, when there is one. Users and API keys need it. */
  deployment?: { id: string; url: string };
  authView?: AuthView;
  onAuthViewChange?: (view: AuthView) => void;
};

export function CapabilityWorkspace(props: Props) {
  if (props.view === "database") {
    return (
      <DatabaseStudio
        host={props.databaseTarget === "deployed" && props.deployedDatabase ? props.deployedDatabase : props.host}
        databaseModel={props.databaseModel}
        onEditSchema={props.onEditSchema}
        onDatabaseModelAccepted={props.onDatabaseModelAccepted}
        onDatabaseModelRestored={props.onDatabaseModelRestored}
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
      <p className="preview-uploads-context">Preview uploads in this browser. Files in the published app are separate.{deployment && <> <a href={deployment.url} target="_blank" rel="noreferrer">Open the published app</a></>}</p>
      <div className="preview-upload-actor"><Field label="Upload as" help={actor === "guest" ? "Choose a preview user to upload a file. Owner sign-in is separate from the app's users." : "Files you upload belong to this preview user."}><Select value={actor} onChange={(event) => { host.setPreviewActor(event.target.value as "guest" | "alice" | "bob"); setError(""); onFilesChanged(); }}><option value="guest">Signed out</option><option value="alice">Alice</option><option value="bob">Bob</option></Select></Field></div>
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
        {files.length === 0 ? <EmptyState onUpload={() => inputRef.current?.click()} disabled={actor === "guest"} text={previewUploadsNote} /> : files.map((file) => (
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
    <section className="cap-workspace" aria-label="Authentication panel">
      <nav className="auth-tabs" aria-label="Auth views">
        <button type="button" className={view === "rules" ? "active" : ""} aria-current={view === "rules" || undefined} onClick={() => show("rules")}><ShieldCheck size={13} /> Rules</button>
        <button type="button" className={view === "users" ? "active" : ""} aria-current={view === "users" || undefined} onClick={() => show("users")}><Users size={13} /> Users</button>
        <button type="button" className={view === "integrations" ? "active" : ""} aria-current={view === "integrations" || undefined} onClick={() => show("integrations")}><Plug size={13} /> Integrations</button>
      </nav>
      {view === "users" && <AppUsersView projectId={props.projectId} deploymentId={props.deployment?.id} visibility={props.visibility ?? "private"} onSetVisibility={props.onSetVisibility} />}
      {view === "integrations" && props.projectId && <IntegrationsView projectId={props.projectId} deploymentId={props.deployment?.id} deploymentUrl={props.deployment?.url} actionsSource={props.actionsSource} />}
      {view === "rules" && <RulesView {...props} />}
    </section>
  );
}

function RulesView({ host, onFilesChanged, actionsSource, onSetAccess, visibility = "private" }: Props) {
  const [actor, setActor] = useState(host.getPreviewActor());
  const refusal = actionsSource ? deployReadiness({ files: { "actions.json": actionsSource }, manifest: { runtime: { database: { actions: [] } } } }).refusal : null;
  const previewName = actor === "alice" ? "a signed-in person" : actor === "bob" ? "another signed-in person" : "someone who has not signed in";
  return (
    <>
      <div className="auth-card">
        <span className="auth-avatar"><UserRound size={27} /></span>
        <div>
          <h3>{authHeading(rules(actionsSource), visibility)}</h3>
          {refusal && <p className="cap-error">actions.json does not parse, so no rule can be shown or changed here. {refusal}</p>}
          <div className="auth-rules">
            {rules(actionsSource).map((action) => (
              <label key={action.name}>
                <span>{actionTitle(action.name)}</span>
                {/* An action whose route is its only door has no access to choose here: its route's callers decide. */}
                {action.http && routeCallers(action.http, visibility) ? <small>{actionSentence(action, visibility)}</small> : <select
                  aria-label={`Who can ${actionTitle(action.name).toLowerCase()}`}
                  value={action.access === "anonymous" ? "anonymous" : "authenticated"}
                  onChange={(event) => onSetAccess?.(action.name, event.target.value === "anonymous" ? "anonymous" : "authenticated")}
                >
                  <option value="anonymous">Anyone</option>
                  <option value="authenticated">A signed-in person</option>
                </select>}
              </label>
            ))}
          </div>
        </div>
      </div>
      <section className="preview-as" aria-label="Preview as">
        <h3>Preview as</h3>
        <label className="preview-user-field"><span>Previewing as {previewName}.</span><select aria-label="Preview user" value={actor} onChange={(event) => {
          const next = event.target.value as "guest" | "alice" | "bob";
          setActor(next);
          host.setPreviewActor(next);
          onFilesChanged();
        }}><option value="guest">Signed out</option><option value="alice">Signed in as Alice</option><option value="bob">Signed in as Bob</option></select></label>
      </section>
      <div className="auth-grid">
        <article><ShieldCheck size={18} /><div><strong>The session belongs only to this app.</strong></div></article>
        <article><KeyRound size={18} /><div><strong>This app decides who that person is. Konae stores the session.</strong></div></article>
      </div>
    </>
  );
}

function EmptyState({ text, onUpload, disabled }: { text: string; onUpload: () => void; disabled?: boolean }) {
  return <UiEmptyState className="cap-empty" icon={<FileImage size={26} />} title="No files yet" description={text} action={<Button variant="primary" disabled={disabled} onClick={onUpload}><HardDriveUpload size={15} /> Upload file</Button>} />;
}

function rules(source: string | undefined) {
  if (!source) return [];
  try {
    return Object.values(parseServerActionSource(source).actions);
  } catch {
    return [];
  }
}

function actionTitle(name: string) {
  const verb = name.split(".").pop() ?? name;
  if (verb === "list") return "List the rows";
  if (verb === "create") return "Add a row";
  if (verb === "toggle") return "Mark a row done";
  return verb.replaceAll("_", " ");
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
