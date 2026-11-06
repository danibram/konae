import { Upload } from "lucide-react";
import { useRef, useState } from "react";
import { importFolder, type ImportReplace, type ImportResult } from "../deployments/import";
import type { SchemaChange } from "../deployments/release";
import { readAppFolder, type ImportedFolder } from "../lib/import-folder";
import { isSignedOutError } from "./owner-session";
import { Button, Radio } from "./ui";

const choices: { value: ImportReplace; label: string; help: string }[] = [
  { value: "both", label: "Sources and data", help: "Publish the folder's app/ and dist/, and replace the database, accounts, files and Usage." },
  { value: "sources", label: "Sources only", help: "Publish the folder's app/ and dist/. The data stays." },
  { value: "data", label: "Data only", help: "Replace the database, accounts, files and Usage. The published sources stay." },
];

type Props = {
  deploymentId: string;
  project: { id: string; name: string; slug: string };
  visibility: "public" | "private";
  onImported: () => void;
  onSignedOut: () => void;
};

/**
 * Imports an exported app folder into this app. The Kernel backs up what
 * the import replaces first, and the Data panel's Backups restores it.
 * Rows are never merged.
 */
export function AppImport({ deploymentId, project, visibility, onImported, onSignedOut }: Props) {
  const [folder, setFolder] = useState<{ name: string; read: ImportedFolder } | null>(null);
  const [replace, setReplace] = useState<ImportReplace>("both");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [losses, setLosses] = useState<SchemaChange | null>(null);
  const [done, setDone] = useState<Extract<ImportResult, { imported: true }> | null>(null);
  const errorRef = useRef<HTMLParagraphElement>(null);

  const choose = async (file: File | undefined) => {
    setError(""); setLosses(null); setDone(null); setFolder(null);
    if (!file) return;
    try {
      const read = readAppFolder(new Uint8Array(await file.arrayBuffer()));
      setFolder({ name: file.name, read });
      if (!read.data.database) setReplace("sources");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      errorRef.current?.focus();
    }
  };

  const run = async (confirmSchema?: string) => {
    if (!folder || busy) return;
    setBusy(true); setError(""); setDone(null);
    try {
      const result = await importFolder({ folder: folder.read, replace, deploymentId, project, visibility, confirmSchema });
      if (!result.imported) setLosses(result.schema);
      else { setLosses(null); setDone(result); }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) onSignedOut();
    } finally { setBusy(false); }
  };

  const hasData = Boolean(folder?.read.data.database);
  return <section className="domain-step" aria-label="Import" aria-busy={busy}>
    <header className="domain-heading"><Upload size={20} /><h3>Import</h3></header>
    <p>Replace this app with an exported app folder (the .tar that Export downloads). A backup of what the import replaces comes first; restore it from the Data panel's Backups. Rows are never merged, and people stay signed in.</p>
    <label className="import-file">
      <span>App folder</span>
      <input type="file" accept=".tar,application/x-tar" disabled={busy} onChange={(event) => void choose(event.target.files?.[0])} />
    </label>
    {folder && <>
      <p>{folder.name}: {folder.read.settings.name}, {Object.keys(folder.read.sources).length} source files{hasData ? ", with data" : ", without data"}.</p>
      <fieldset className="import-choices" disabled={busy}>
        <legend>Replace</legend>
        {choices.map((choice) => <Radio key={choice.value} name="import-replace" value={choice.value} checked={replace === choice.value} disabled={choice.value !== "sources" && !hasData}
          onChange={() => { setReplace(choice.value); setLosses(null); setDone(null); }}
          label={<span><strong>{choice.label}</strong> <small>{choice.help}</small></span>} />)}
      </fieldset>
      {!losses && <Button variant="primary" loading={busy} onClick={() => void run()}>Import</Button>}
    </>}
    {losses && <section className="domain-confirm" aria-label="Confirm data loss">
      <h4>This import deletes data</h4>
      <ul>{losses.losses.map((loss) => <li key={loss.sentence}>{loss.sentence}</li>)}</ul>
      <div className="domain-actions">
        <Button onClick={() => setLosses(null)} disabled={busy}>Keep the data</Button>
        <Button variant="danger" loading={busy} onClick={() => void run(losses.confirmation ?? undefined)}>Import and delete</Button>
      </div>
    </section>}
    {error && <p className="domain-error" role="alert" tabIndex={-1} ref={errorRef}>{error}</p>}
    {done && <div className="domain-notice" role="status">
      <p>Imported {done.replace === "both" ? "the sources and the data" : done.replace === "sources" ? "the sources" : "the data"}. Backup "{done.backup.label}" from {new Date(done.backup.createdAt).toLocaleString()} holds what it replaced.</p>
      <Button onClick={onImported}>Reload the editor</Button>
    </div>}
  </section>;
}
