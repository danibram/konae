import {
  File,
  FileImage,
  HardDriveUpload,
  KeyRound,
  ShieldCheck,
  UserRound,
} from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { DatabaseAdminHost, MiniAppRuntimeHost, StoredFile } from "../runtime/host";
import type { DrizzleDatabaseModel } from "../lib/types";
import { HostedDatabaseClient, readSignupPolicy, saveSignupPolicy, type SignupMode, type SignupPolicy } from "../deployments/client";
import { parseServerActionSource } from "../kernel/actions";
import { McpError } from "../mcp/client";
import { Button, Card, Field, Select, Textarea, EmptyState as UiEmptyState } from "./ui";
import { DatabaseStudio } from "./DatabaseStudio";

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

function FilesPanel({ host, onFilesChanged }: Props) {
  const [files, setFiles] = useState<StoredFile[]>(host.getFiles());
  const [error, setError] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  const upload = async (file?: globalThis.File) => {
    if (!file) return;
    setError("");
    try {
      const dataUrl = await readFile(file);
      await host.handle("files.upload", { name: file.name, type: file.type, size: file.size, dataUrl });
      setFiles(host.getFiles());
      onFilesChanged();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  return (
    <section className="cap-workspace" aria-label="Uploads">
      <input ref={inputRef} className="sr-only" type="file" onChange={(event) => { void upload(event.target.files?.[0]); event.target.value = ""; }} />
      {files.length > 0 && (
        <div className="files-summary">
          <span>{files.length} upload{files.length === 1 ? "" : "s"}</span>
          <span>{formatBytes(files.reduce((sum, file) => sum + file.size, 0))}</span>
          <Button variant="primary" className="files-upload" onClick={() => inputRef.current?.click()}><HardDriveUpload size={13} /> Upload file</Button>
        </div>
      )}
      {error && <div className="cap-error">{error}</div>}
      <div className="file-grid">
        {files.length === 0 ? <EmptyState onUpload={() => inputRef.current?.click()} text="Add one when the app needs it, a logo or a CSV. Export takes it along." /> : files.map((file) => (
          <article key={file.id} className="file-card">
            <div className="file-preview">{file.type.startsWith("image/") ? <img src={file.url} alt="" /> : <File size={27} />}</div>
            <div><strong>{file.name}</strong><small>{formatBytes(file.size)} · {file.type || "unknown"}</small></div>
          </article>
        ))}
      </div>
    </section>
  );
}

function AuthPanel({ host, onFilesChanged, deployedDatabase, actionsSource, onSetAccess, visibility = "public", onSetVisibility }: Props) {
  const [actor, setActor] = useState(host.getPreviewActor());
  const previewName = actor === "alice" ? "a signed-in person" : actor === "bob" ? "another signed-in person" : "someone who has not signed in";
  return (
    <section className="cap-workspace" aria-label="Authentication panel">
      <div className="auth-card">
        <span className="auth-avatar"><UserRound size={27} /></span>
        <div>
          <h3>Anyone can list the rows. The list stops at 100 rows.</h3>
          <label className="auth-open">
            <span>Who can open the app</span>
            <select aria-label="Who can open the app" value={visibility} onChange={(event) => onSetVisibility?.(event.target.value === "private" ? "private" : "public")}>
              <option value="public">Anyone with the link</option>
              <option value="private">A signed-in person</option>
            </select>
          </label>
          <div className="auth-rules">
            {rules(actionsSource).map((action) => (
              <label key={action.name}>
                <span>{actionTitle(action.name)}</span>
                <select
                  aria-label={`Who can ${actionTitle(action.name).toLowerCase()}`}
                  value={action.access === "anonymous" ? "anonymous" : "authenticated"}
                  onChange={(event) => onSetAccess?.(action.name, event.target.value === "anonymous" ? "anonymous" : "authenticated")}
                >
                  <option value="anonymous">Anyone</option>
                  <option value="authenticated">A signed-in person</option>
                </select>
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
        }}><option value="guest">Someone who has not signed in</option><option value="alice">A signed-in person</option><option value="bob">Another signed-in person</option></select></label>
      </section>
      <SignupPolicyCard deploymentId={deployedDatabase instanceof HostedDatabaseClient ? deployedDatabase.deploymentId : undefined} />
      <div className="auth-grid">
        <article><ShieldCheck size={18} /><div><strong>The session belongs only to this app.</strong></div></article>
        <article><KeyRound size={18} /><div><strong>This app decides who that person is. Littlebox stores the session.</strong></div></article>
      </div>
    </section>
  );
}

const signupModes: { value: SignupMode; label: string; help: string }[] = [
  { value: "open", label: "Open", help: "Anyone who opens the published app can create an account." },
  { value: "allowlist", label: "Allowlist (emails or @domains)", help: "Only the emails and domains listed below can create an account. Littlebox does not verify email addresses, so whoever signs up first with a listed address gets it; choose Closed for private data." },
  { value: "closed", label: "Closed (no new accounts)", help: "No one can create an account. Existing accounts can still sign in." },
];

type PolicyState =
  | { status: "loading" }
  | { status: "signed-out" }
  | { status: "failed"; message: string }
  | { status: "ready"; policy: SignupPolicy };

/** Who may create an account on the deployed app. The Kernel enforces it; the preview has fixed users. */
function SignupPolicyCard({ deploymentId }: { deploymentId?: string }) {
  const [state, setState] = useState<PolicyState>({ status: "loading" });
  const [mode, setMode] = useState<SignupMode>("open");
  const [entries, setEntries] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (!deploymentId) return;
    let current = true;
    setState({ status: "loading" });
    readSignupPolicy(deploymentId).then((policy) => {
      if (!current) return;
      setState({ status: "ready", policy });
      setMode(policy.mode);
      setEntries(policy.allowlist.join("\n"));
    }, (cause) => {
      if (!current) return;
      setState(cause instanceof McpError && cause.status === 401 ? { status: "signed-out" } : { status: "failed", message: cause instanceof Error ? cause.message : String(cause) });
    });
    return () => { current = false; };
  }, [deploymentId]);

  let body: ReactNode;
  if (!deploymentId) {
    body = <>
      <Field label="Who can create an account" help="People who use the app create an account here. Deploy saves this choice.">
        <Select value={mode} onChange={(event) => setMode(event.target.value as SignupMode)}>
          {signupModes.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        </Select>
      </Field>
    </>;
  } else if (state.status === "loading") {
    body = <p className="preview-user-help">Loading the signup policy.</p>;
  } else if (state.status === "signed-out") {
    body = <p className="preview-user-help">Sign in with <a href="/_littlebox/owner/login">owner sign-in</a> to manage who can create accounts.</p>;
  } else if (state.status === "failed") {
    body = <div className="cap-error">{state.message}</div>;
  } else {
    const list = entries.split(/[\n,]/).map((entry) => entry.trim()).filter(Boolean);
    const unchanged = mode === state.policy.mode && list.join("\n") === state.policy.allowlist.join("\n");
    const save = async () => {
      setSaving(true);
      setError("");
      setSaved(false);
      try {
        const policy = await saveSignupPolicy(deploymentId, mode, list);
        setState({ status: "ready", policy });
        setMode(policy.mode);
        setEntries(policy.allowlist.join("\n"));
        setSaved(true);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setSaving(false);
      }
    };
    body = <>
      <Field label="Who can create an account" help={signupModes.find((option) => option.value === mode)?.help}>
        <Select value={mode} onChange={(event) => { setMode(event.target.value as SignupMode); setSaved(false); }}>
          {signupModes.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        </Select>
      </Field>
      {mode === "allowlist" && <Field label="Allowed emails and domains" help="One per line, for example ana@example.com or @example.com. Up to 100 entries.">
        <Textarea rows={4} value={entries} aria-invalid={Boolean(error) || undefined} onChange={(event) => { setEntries(event.target.value); setSaved(false); }} />
      </Field>}
      {mode === "allowlist" && list.length === 0 && <p className="preview-user-help">With an empty list no one can create an account.</p>}
      {error && <div className="cap-error">{error}</div>}
      <div className="signup-policy-actions">
        <Button variant="primary" size="sm" loading={saving} disabled={unchanged} onClick={() => void save()}>Save signup policy</Button>
        <span role="status">{saved ? "Saved. The published app uses it now." : ""}</span>
      </div>
    </>;
  }

  return (
    <Card className="signup-policy" role="group" aria-label="Published app signup">
      <p>Accounts</p>
      {body}
    </Card>
  );
}

function EmptyState({ text, onUpload }: { text: string; onUpload: () => void }) {
  return <UiEmptyState className="cap-empty" icon={<FileImage size={26} />} title="No files yet" description={text} action={<Button variant="primary" onClick={onUpload}><HardDriveUpload size={15} /> Upload file</Button>} />;
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
