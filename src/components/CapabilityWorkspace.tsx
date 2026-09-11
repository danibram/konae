import {
  File,
  FileImage,
  HardDriveUpload,
  KeyRound,
  ShieldCheck,
  UserRound,
} from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { DatabaseAdminHost, MiniAppRuntimeHost, RuntimeSession, StoredFile } from "../runtime/host";
import type { DrizzleDatabaseModel } from "../lib/types";
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
    <section className="cap-workspace" aria-label="Files panel">
      <input ref={inputRef} className="sr-only" type="file" onChange={(event) => { void upload(event.target.files?.[0]); event.target.value = ""; }} />
      <div className="files-summary">
        <span>{files.length} object{files.length === 1 ? "" : "s"}</span>
        <span>{formatBytes(files.reduce((sum, file) => sum + file.size, 0))} used</span>
        <span>2 MiB/file local limit</span>
        <button className="files-upload" onClick={() => inputRef.current?.click()}><HardDriveUpload size={13} /> Upload file</button>
      </div>
      {error && <div className="cap-error">{error}</div>}
      <div className="file-grid">
        {files.length === 0 ? <EmptyState text="No files yet. Upload one here or from the mini-app pinboard." /> : files.map((file) => (
          <article key={file.id} className="file-card">
            <div className="file-preview">{file.type.startsWith("image/") ? <img src={file.url} alt="" /> : <File size={27} />}</div>
            <div><strong>{file.name}</strong><small>{formatBytes(file.size)} · {file.type || "unknown"}</small></div>
          </article>
        ))}
      </div>
    </section>
  );
}

function AuthPanel({ host, onFilesChanged }: Props) {
  const [session, setSession] = useState<RuntimeSession>(() => host.getSession());
  useEffect(() => host.subscribeSession(setSession), [host]);
  return (
    <section className="cap-workspace" aria-label="Authentication panel">
      <div className="auth-card">
        <span className="auth-avatar"><UserRound size={27} /></span>
        <div><p>CURRENT SESSION</p><h3>{session.name}</h3><code>{session.userId}</code></div>
        <span className="auth-role">{session.role}</span>
      </div>
      <label>Preview user <select aria-label="Preview user" value={host.getPreviewActor()} onChange={(event) => {
        host.setPreviewActor(event.target.value as "guest" | "alice" | "bob");
        onFilesChanged();
      }}><option value="guest">Guest</option><option value="alice">Alice</option><option value="bob">Bob</option></select></label>
      <p>These users simulate access in the preview. Published apps have their own signup and login.</p>
      <div className="auth-grid">
        <article><ShieldCheck size={18} /><div><strong>Origin bound</strong><p>The session belongs only to this mini-app.</p></div></article>
        <article><KeyRound size={18} /><div><strong>Kernel-owned identity</strong><p>Providers store sessions; Littlebox decides identity and grants.</p></div></article>
      </div>
      <div className="provider-list">
        <p>PROVIDERS</p>
        <div><span className="provider-mark guest">K</span><strong>Littlebox Kernel</strong><small>Enabled</small></div>
        <div className="disabled"><span className="provider-mark">O</span><strong>OAuth adapter</strong><small>Optional input</small></div>
        <div className="disabled"><span className="provider-mark">@</span><strong>Magic link module</strong><small>Planned</small></div>
      </div>
    </section>
  );
}

function EmptyState({ text }: { text: string }) {
  return <div className="cap-empty"><FileImage size={25} /><p>{text}</p></div>;
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
