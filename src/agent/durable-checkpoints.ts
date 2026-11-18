import { sourceBytes } from "../lib/source-files";
import type { SourceFiles } from "../lib/source-files";
import type { DurableCheckpoint, DurableCheckpointDraft } from "./durable-storage";

export const DURABLE_CHECKPOINT_FORMAT = 1;
export type CheckpointFiles = SourceFiles;
export type CheckpointStatus = "working" | "proposal" | "applied-unsaved" | "saved" | "discarded" | "stale" | "apply-unknown";

export type CheckpointInput = {
  projectId: string;
  sourcesRevision: number;
  base: CheckpointFiles;
  scratch: CheckpointFiles;
  modelView: CheckpointFiles;
  /** The exact model-visible entries at this boundary, including compaction records. */
  modelContext: unknown;
  nativeSequence: number;
  validationSettings: Record<string, unknown>;
  historicalGate?: unknown;
  status: CheckpointStatus;
  parentCheckpointId?: string;
  candidateId?: string;
  lifecycle?: Record<string, unknown>;
};

export type CheckpointRead = {
  id: string;
  sequence: number;
  sourcesRevision: number;
  status: CheckpointStatus;
  base: CheckpointFiles;
  scratch: CheckpointFiles;
  modelView: CheckpointFiles;
  modelContext: unknown;
  historicalGate?: unknown;
  validationSettings: Record<string, unknown>;
  parentCheckpointId?: string;
  candidateId?: string;
  lifecycle?: Record<string, unknown>;
  stale: boolean;
  applyAllowed: false;
  readable: boolean;
  error?: string;
};

/** Read only the saved source and candidate file sets for keyless owner inspection.
 * This deliberately does not reconstruct model context, claim a lease or contact a provider. */
export async function inspectCheckpointCandidate(checkpoint: DurableCheckpoint, readBlob: (digest: string) => Promise<Uint8Array>) {
  const payload = checkpoint.payload as any;
  if (!payload || payload.formatVersion !== DURABLE_CHECKPOINT_FORMAT) throw new Error(`Checkpoint format ${String(payload?.formatVersion)} cannot be inspected.`);
  const refs = payload.blobFiles as Record<string, string> | undefined;
  if (!refs || typeof refs !== "object" || !payload.base?.manifest || !payload.scratch?.manifest) throw new Error("The saved proposal file references are malformed.");
  const byteTotal = [payload.base, payload.scratch].reduce((total: number, set: any) => total + Object.values(set.manifest as Record<string, { byteLength?: number }>).reduce((sum, file) => sum + (Number.isSafeInteger(file.byteLength) ? file.byteLength! : 0), 0), 0);
  if (byteTotal > 2 * 1024 * 1024) throw new Error("Saved proposal comparison exceeds the 2 MiB keyless inspection limit.");
  const cache = new Map<string, Promise<Uint8Array>>();
  const cachedRead = (digest: string) => {
    let pending = cache.get(digest);
    if (!pending) { pending = readBlob(digest); cache.set(digest, pending); }
    return pending;
  };
  const [base, scratch] = await Promise.all([decodeFiles(payload.base, refs, cachedRead), decodeFiles(payload.scratch, refs, cachedRead)]);
  return { id: checkpoint.checkpointId, sequence: checkpoint.sequence, sourcesRevision: payload.sourcesRevision as number, status: payload.status as CheckpointStatus, base, scratch };
}

/** Create a checkpoint draft. The native commit adapter uploads every byte before atomically
 * committing this metadata with its Pi batch; a failed upload or commit exposes no checkpoint. */
export async function createCheckpointDraft(input: CheckpointInput): Promise<DurableCheckpointDraft> {
  const blobs: Array<{ key: string; bytes: Uint8Array }> = [];
  const fileSets = await Promise.all([
    encodeFiles("base", input.base, blobs),
    encodeFiles("scratch", input.scratch, blobs),
    encodeFiles("modelView", input.modelView, blobs),
  ]);
  const payload = {
    formatVersion: DURABLE_CHECKPOINT_FORMAT,
    projectId: input.projectId,
    sourcesRevision: input.sourcesRevision,
    base: fileSets[0], scratch: fileSets[1], modelView: fileSets[2],
    modelContext: { sequence: input.nativeSequence, kind: "native-prefix" },
    validationSettings: input.validationSettings,
    ...(input.historicalGate === undefined ? {} : { historicalGate: input.historicalGate }),
    status: input.status,
    ...(input.parentCheckpointId ? { parentCheckpointId: input.parentCheckpointId } : {}),
    ...(input.candidateId ? { candidateId: input.candidateId } : {}),
    ...(input.lifecycle ? { lifecycle: input.lifecycle } : {}),
    blobs: [] as string[],
  };
  return { payload, blobs };
}

async function encodeFiles(prefix: string, files: CheckpointFiles, blobs: Array<{ key: string; bytes: Uint8Array }>) {
  const manifest: Record<string, { encoding: "utf8" | "base64"; digest: string; byteLength: number; blobKey: string }> = Object.create(null);
  for (const path of Object.keys(files.files).sort()) {
    const bytes = sourceBytes(files, path as never);
    const key = `${prefix}:${path}`;
    blobs.push({ key, bytes });
    manifest[path] = { encoding: files.encodings?.[path as never] === "base64" ? "base64" : "utf8", digest: await digest(bytes), byteLength: bytes.byteLength, blobKey: key };
  }
  return { manifest, encodings: files.encodings ?? {} };
}

export async function recoverCheckpoint(
  checkpoint: DurableCheckpoint,
  readBlob: (digest: string) => Promise<Uint8Array>,
  currentSources: { revision: number; files: CheckpointFiles },
  readModelContext: (throughSequence: number) => Promise<unknown>,
): Promise<CheckpointRead> {
  const p = checkpoint.payload as any;
  const revision = typeof p?.sourcesRevision === "number" ? p.sourcesRevision : -1;
  const basic = { id: checkpoint.checkpointId, sequence: checkpoint.sequence, sourcesRevision: revision, status: p?.status as CheckpointStatus, historicalGate: p?.historicalGate, validationSettings: p?.validationSettings ?? {}, parentCheckpointId: p?.parentCheckpointId, candidateId: p?.candidateId, lifecycle: p?.lifecycle, stale: revision !== currentSources.revision, applyAllowed: false as const };
  if (!p || typeof p !== "object" || p.formatVersion !== DURABLE_CHECKPOINT_FORMAT) return { ...basic, base: { files: {} }, scratch: { files: {} }, modelView: { files: {} }, modelContext: null, readable: false, error: `Checkpoint format ${String(p?.formatVersion)} is unsupported or malformed; this history is preserved for export.` };
  try {
    const refs = p.blobFiles as Record<string, string>;
    if (!Number.isSafeInteger(p.sourcesRevision) || p.sourcesRevision < 0 || !Array.isArray(p.blobs) || !["working", "proposal", "applied-unsaved", "saved", "discarded", "stale", "apply-unknown"].includes(p.status)) throw new Error("Checkpoint source revision or lifecycle status is malformed.");
    if (!refs || typeof refs !== "object" || !p.modelContext || p.modelContext.sequence !== checkpoint.sequence || p.modelContext.kind !== "native-prefix") throw new Error("Checkpoint references or model context boundary are malformed.");
    const readCache = new Map<string, Promise<Uint8Array>>();
    const cachedRead = (digestValue: string) => {
      let pending = readCache.get(digestValue);
      if (!pending) { pending = readBlob(digestValue); readCache.set(digestValue, pending); }
      return pending;
    };
    const base = await decodeFiles(p.base, refs, cachedRead);
    const scratch = await decodeFiles(p.scratch, refs, cachedRead);
    const modelView = await decodeFiles(p.modelView, refs, cachedRead);
    let modelContext: unknown;
    try { modelContext = await readModelContext(p.modelContext.sequence); }
    catch (error) {
      return { ...basic, base, scratch, modelView, modelContext: null, readable: false, error: error instanceof Error ? error.message : String(error) };
    }
    if (!modelContext || typeof modelContext !== "object" || (modelContext as any).sequence !== p.modelContext.sequence || !Array.isArray((modelContext as any).messages)) {
      return { ...basic, base, scratch, modelView, modelContext: null, readable: false, error: "The immutable native model context prefix could not be reconstructed at this checkpoint." };
    }
    return { ...basic, base, scratch, modelView, modelContext, readable: true };
  } catch (error) {
    return { ...basic, base: { files: {} }, scratch: { files: {} }, modelView: { files: {} }, modelContext: null, readable: false, error: error instanceof Error ? error.message : String(error) };
  }
}

export async function reconcileInterruptedApply(
  checkpoint: Pick<CheckpointRead, "sourcesRevision" | "base" | "scratch">,
  observed: { revision: number; files: CheckpointFiles } | null,
): Promise<"saved" | "unchanged" | "conflict" | "unknown"> {
  if (!observed) return "unknown";
  if (observed.revision === checkpoint.sourcesRevision + 1 && await sameFiles(observed.files, checkpoint.scratch)) return "saved";
  if (observed.revision === checkpoint.sourcesRevision && await sameFiles(observed.files, checkpoint.base)) return "unchanged";
  return "conflict";
}

export function proposalDisposition(status: CheckpointStatus, files: CheckpointFiles) {
  const paths = Object.keys(files.files).sort();
  if (status === "discarded") return `The prior proposal was discarded. Its candidate files (${paths.join(", ") || "no files"}) were rejected and remain unchanged in the saved Project. Do not treat them as accepted; use the current saved files for this request.`;
  if (status === "applied-unsaved") return `The prior proposal was accepted into the editor files (${paths.join(", ") || "no files"}) but has not been confirmed saved to the installation. The saved source revision remains separate until read-back confirms it.`;
  if (status === "apply-unknown") return `The prior Apply outcome is unobserved. Do not assume the candidate was saved or rejected, and do not repeat the write. The current saved sources must be read back before making another Apply decision.`;
  if (status !== "saved") return `The prior proposal is still ${status}. Its files are not confirmed saved; continue only from the current observed source revision.`;
  return `The prior proposal was saved and read back from the installation at its recorded source revision. Current saved files include: ${paths.join(", ") || "no files"}.`;
}

async function sameFiles(left: CheckpointFiles, right: CheckpointFiles) {
  const leftPaths = Object.keys(left.files).sort();
  const rightPaths = Object.keys(right.files).sort();
  if (JSON.stringify(leftPaths) !== JSON.stringify(rightPaths)) return false;
  for (const path of leftPaths) {
    if ((left.encodings?.[path as never] === "base64") !== (right.encodings?.[path as never] === "base64")) return false;
    if (await digest(sourceBytes(left, path as never)) !== await digest(sourceBytes(right, path as never))) return false;
  }
  return true;
}

async function decodeFiles(value: any, refs: Record<string, string>, readBlob: (digest: string) => Promise<Uint8Array>): Promise<CheckpointFiles> {
  const files: Record<string, string> = Object.create(null);
  const encodings: Record<string, "base64"> = Object.create(null);
  for (const [path, file] of Object.entries(value.manifest as Record<string, any>)) {
    const digestValue = refs[file.blobKey];
    if (!digestValue) throw new Error(`The checkpoint file reference for ${path} is missing.`);
    const bytes = await readBlob(digestValue);
    if (bytes.byteLength !== file.byteLength || await digest(bytes) !== file.digest) throw new Error(`The checkpoint file ${path} failed its digest check.`);
    if (file.encoding === "base64") {
      files[path] = toBase64(bytes);
      encodings[path] = "base64";
    } else files[path] = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  }
  return { files, ...(Object.keys(encodings).length ? { encodings } : {}) };
}

function toBase64(bytes: Uint8Array) {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  return btoa(binary);
}

async function digest(bytes: Uint8Array) {
  const value = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes.slice().buffer as ArrayBuffer));
  return [...value].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
