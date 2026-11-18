import { useState } from "react";
import { attachLocalHistory, type LocalHistoryAttachment } from "../agent/durable-storage";
import type { MiniAppProject } from "../lib/types";
import type { ProjectSources } from "../lib/project-sources";
import { Button, Dialog } from "./ui";

export function LocalHistoryAttachmentDialog({ attachment, projects, sources, ownerId, onClose, onAttached }: {
  attachment: unknown;
  projects: MiniAppProject[];
  sources: ProjectSources;
  ownerId: string;
  onClose: () => void;
  onAttached: (conversationId: string) => void;
}) {
  const [projectId, setProjectId] = useState(projects[0]?.id ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const value = validateAttachment(attachment);
  const project = projects.find((item) => item.id === projectId);
  const fileNames = [...new Set([...Object.keys(value?.files.current ?? {}), ...Object.keys(value?.files.candidate ?? {})])].sort();
  const attach = async () => {
    if (!value || !project) return;
    setBusy(true); setError("");
    try {
      const observed = await sources.observeSaved(project.id);
      if (!observed.project) throw new Error("The selected owned Project’s saved sources could not be read. Nothing was attached.");
      const result = await attachLocalHistory({
        project: { ...observed.project, revision: observed.revision }, ownerId, attachment: value,
      });
      if (result.copiedEntries !== value.records.length) throw new Error("The owner read-back did not confirm every copied entry. Keep this local handoff and reconcile before retrying.");
      onAttached(result.conversation.id);
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  };
  return <Dialog title="Review local attachment" description="Choose an owned saved Project. This copies a local conversation and file checkpoint; it never saves sources, applies files, or deploys." onClose={onClose} className="durable-history-dialog" footer={<><Button onClick={onClose} disabled={busy}>Cancel</Button><Button variant="primary" onClick={() => void attach()} disabled={busy || !value || !project}>{busy ? "Attaching…" : "Attach this copy"}</Button></>}>
    <div className="ui-dialog-body">
      {!value && <p role="alert">The local handoff is missing required fields or has an unsupported version. No data was sent.</p>}
      {value && <>
        <label>Owned Project<select className="select" value={projectId} onChange={(event) => setProjectId(event.target.value)}>{projects.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>
        <p>Will copy {value.records.length} saved conversation entries and {fileNames.length} local files. The latest proposal files remain a separate candidate from the selected Project’s freshly read saved sources.</p>
        <ul>{fileNames.map((name) => <li key={name}><code>{name}</code>{value.files.candidate?.[name] !== undefined && value.files.current[name] !== value.files.candidate[name] ? " · proposal candidate" : " · local app file"}</li>)}</ul>
        <p>No provider key is copied. No source save, Apply, or Deploy happens here. The attachment is a new conversation in the selected Project.</p>
        {value.continuation.reasons.length > 0 && <div role="note"><strong>Inspection only.</strong> This format cannot preserve all native pending work or pre-attachment tool details, so the copy keeps its messages and files but cannot be continued. {value.continuation.reasons.join(" ")}</div>}
      </>}
      {error && <p role="alert">{error}</p>}
    </div>
  </Dialog>;
}

function validateAttachment(input: unknown): LocalHistoryAttachment | null {
  if (!input || typeof input !== "object") return null;
  const value = input as Record<string, any>;
  if (value.version !== 1 || typeof value.attachmentId !== "string" || value.attachmentId.length > 64 || !value.recordIdMap || typeof value.recordIdMap !== "object" || !Array.isArray(value.records) || !value.files || typeof value.files.current !== "object" || !value.modelContext || !Array.isArray(value.modelContext.messages) || typeof value.continuation?.eligible !== "boolean" || !Array.isArray(value.continuation.reasons)) return null;
  const mappedIds = value.records.map((entry: Record<string, unknown>) => value.recordIdMap[String(entry.id)]);
  if (mappedIds.some((id: unknown) => !Number.isSafeInteger(id) || (id as number) <= 1) || new Set(mappedIds).size !== mappedIds.length) return null;
  const byteLength = new TextEncoder().encode(JSON.stringify(value)).byteLength;
  if (byteLength > 4 * 1024 * 1024 || value.records.length > 10_000) return null;
  return value as LocalHistoryAttachment;
}
