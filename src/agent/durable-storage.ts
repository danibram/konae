import type { Context } from "@earendil-works/chord/context";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { MemoryStorage } from "@earendil-works/pi-durable/storage/memory";
import { ROOT_CONVERSATION_ID } from "@earendil-works/pi-durable";
import type { Storage, StorageWrite } from "@earendil-works/pi-durable";

/** Browser transport for Konae's authenticated native Pi batch API. */
export type DurableConversationInfo = {
  id: string;
  formatVersion: number;
  piVersion: string;
  registryVersion: string;
  title: string;
  lastSeq: number;
  generation: number;
  createdAt?: number;
  updatedAt?: number;
};
export type DurableConversationCursor = { before: number; beforeId: string };
export type DurableRecordCursor = { before: number; beforeOrdinal: number };
export type DurableRecordSummary = { kind: string; messages?: Array<{ role?: string; content?: Array<{ text: string; clipped?: boolean }>; toolCalls?: Array<{ id?: string; name?: string; timestamp?: number }>; toolCallId?: string; toolName?: string; isError?: boolean; stopReason?: string; timestamp?: number }>; intent?: unknown };
export type DurableRecordSummaryItem = { sequence: number; ordinal: number; id: number | null; type: string; formatVersion: number; summary: DurableRecordSummary };
export type DurableCheckpointSummary = { checkpointId: string; sequence: number; createdAt: number; status: string | null; sourcesRevision: number | null; candidateId: string | null; inspectionOnly?: boolean };

function ownerHeaders(ownerId?: string) { return ownerId ? { "x-konae-owner-id": ownerId } : undefined; }

export async function listDurableConversations(projectId: string, ownerId?: string, cursor?: DurableConversationCursor, fetcher: typeof fetch = fetch) {
  const query = new URLSearchParams({ limit: "50" });
  if (cursor) { query.set("before", String(cursor.before)); query.set("before_id", cursor.beforeId); }
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations?${query}`, { credentials: "same-origin", cache: "no-store", headers: ownerHeaders(ownerId) });
  if (!response.ok) throw new Error(`Could not read recent conversations (${response.status}).`);
  return await response.json() as { items: DurableConversationInfo[]; nextCursor: DurableConversationCursor | null };
}

export async function readDurableRecords(projectId: string, conversationId: string, ownerId?: string, cursor?: DurableRecordCursor, fetcher: typeof fetch = fetch) {
  const query = new URLSearchParams({ limit: "50" });
  if (cursor) { query.set("before", String(cursor.before)); query.set("beforeOrdinal", String(cursor.beforeOrdinal)); }
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations/${encodeURIComponent(conversationId)}/records?${query}`, { credentials: "same-origin", cache: "no-store", headers: ownerHeaders(ownerId) });
  if (!response.ok) throw new Error(`Could not read conversation history (${response.status}).`);
  return await response.json() as { items: DurableRecordSummaryItem[]; nextCursor: DurableRecordCursor | null };
}

export async function readDurableRecordIds(projectId: string, conversationId: string, ownerId?: string, cursor?: DurableRecordCursor, fetcher: typeof fetch = fetch) {
  const query = new URLSearchParams({ limit: "50", idsOnly: "true" });
  if (cursor) { query.set("before", String(cursor.before)); query.set("beforeOrdinal", String(cursor.beforeOrdinal)); }
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations/${encodeURIComponent(conversationId)}/records?${query}`, { credentials: "same-origin", cache: "no-store", headers: ownerHeaders(ownerId) });
  if (!response.ok) throw new Error(`Could not verify native conversation entry IDs (${response.status}).`);
  return await response.json() as { items: Array<{ sequence: number; ordinal: number; id: number | null; type: string }>; nextCursor: DurableRecordCursor | null };
}

export async function readDurableCheckpointSummary(projectId: string, conversationId: string, ownerId?: string, fetcher: typeof fetch = fetch) {
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations/${encodeURIComponent(conversationId)}/checkpoints?summary=true`, { credentials: "same-origin", cache: "no-store", headers: ownerHeaders(ownerId) });
  if (!response.ok) throw new Error(`Could not read the latest proposal checkpoint (${response.status}).`);
  return await response.json() as { item: DurableCheckpointSummary | null };
}

export async function readDurableRecord(projectId: string, conversationId: string, recordId: number, ownerId?: string, fetcher: typeof fetch = fetch) {
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations/${encodeURIComponent(conversationId)}/records/${recordId}/details`, { credentials: "same-origin", cache: "no-store", headers: ownerHeaders(ownerId) });
  if (!response.ok) throw new Error(`Could not read expanded conversation entry (${response.status}).`);
  return await response.json() as Record<string, unknown>;
}

export async function deleteDurableConversation(projectId: string, conversationId: string, ownerId?: string, fetcher: typeof fetch = fetch) {
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations/${encodeURIComponent(conversationId)}`, { method: "DELETE", credentials: "same-origin", cache: "no-store", headers: ownerHeaders(ownerId) });
  if (!response.ok) throw new Error(`Could not delete conversation (${response.status}).`);
  return await response.json() as { deleted: boolean; conversationId: string };
}

export async function downloadDurableConversation(projectId: string, conversationId: string, ownerId?: string, fetcher: typeof fetch = fetch) {
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations/${encodeURIComponent(conversationId)}/download`, { credentials: "same-origin", cache: "no-store", headers: ownerHeaders(ownerId) });
  if (!response.ok) throw new Error(`Could not download conversation (${response.status}).`);
  return await response.blob();
}
export type DurableExecutionLease = { generation: number; conversationId?: string | null; executorId: string | null; expiresAt: number | null; live: boolean; unresolvedEffects: boolean; stopRequested?: boolean; serverNow: number };
export type DurableEffectReceipt = { callId: string; executorId: string; generation: number; tool: string; effectClass: string; state: string; observationCallId: string | null; createdAt: number; updatedAt: number; intent?: { origin?: string; sourceEntryId?: string; text?: string; declaredAt?: number } | null; refusal?: string | null };

export async function readDurableLease(projectId: string, conversationId: string, ownerId?: string, fetcher: typeof fetch = fetch): Promise<DurableExecutionLease> {
  const headers: Record<string,string> = {};
  if (ownerId) headers["x-konae-owner-id"] = ownerId;
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations/${encodeURIComponent(conversationId)}/lease`, { credentials:"same-origin", cache:"no-store", headers });
  if (!response.ok) throw new Error(`Could not inspect project execution lease (${response.status}).`);
  return await response.json() as DurableExecutionLease;
}

/** Ask the currently live browser for a cooperative stop, drain and release. */
export async function requestDurableStop(projectId: string, conversationId: string, generation: number, requestId = crypto.randomUUID(), ownerId?: string, fetcher: typeof fetch = fetch): Promise<{ requested: boolean; requestId: string; executorId: string; generation: number }> {
  const headers: Record<string,string> = { "content-type": "application/json" };
  if (ownerId) headers["x-konae-owner-id"] = ownerId;
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations/${encodeURIComponent(conversationId)}/stop`, { method:"POST", credentials:"same-origin", cache:"no-store", headers, body:JSON.stringify({ requestId, generation }) });
  if (!response.ok) throw new Error(`Could not request project execution stop (${response.status}).`);
  return await response.json() as { requested: boolean; requestId: string; executorId: string; generation: number };
}

export async function readDurableEffects(projectId: string, conversationId: string, ownerId?: string, fetcher: typeof fetch = fetch): Promise<DurableEffectReceipt[]> {
  const headers: Record<string,string> = {};
  if (ownerId) headers["x-konae-owner-id"] = ownerId;
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations/${encodeURIComponent(conversationId)}/effects`, { credentials:"same-origin", cache:"no-store", headers });
  if (!response.ok) throw new Error(`Could not inspect project effect receipts (${response.status}).`);
  return (await response.json() as { items: DurableEffectReceipt[] }).items;
}

export type DurableStorageOptions = {
  projectId: string;
  conversationId: string;
  ownerId?: string;
  executorId?: string;
  fetch?: typeof fetch;
  /** Previously inspected immutable prefix. Writer claim still rechecks the live high-water mark. */
  nativeSnapshot?: readonly { sequence: number; writes: StorageWrite[] }[];
  localOnly?: boolean;
  /** Adds a checkpoint atomically with each batch. Build it from captured state; never await this Harness's context here. */
  checkpoint?: (writes: readonly StorageWrite[], sequence: number) => Promise<DurableCheckpointDraft | null>;
  /** Advances the caller's inspected boundary only after the native receipt and local apply are complete. */
  onAcknowledged?: (sequence: number) => void;
  onLeaseLost?: (error: unknown) => void;
};

export type DurableCheckpointDraft = {
  id?: string;
  payload: Record<string, unknown>;
  blobs?: readonly { key: string; bytes: Uint8Array }[];
};
export type DurableCheckpoint = { checkpointId: string; sequence: number; payload: Record<string, unknown>; createdAt: number };

type NativeReceipt = { requestId: string; stored: number; sequences: number[]; lastSeq: number; refused: string | null; checkpointStored?: boolean };
type NativeBatch = { sequence: number; writes: StorageWrite[] };
export type NativeBatchSnapshot = readonly NativeBatch[];

/**
 * The local MemoryStorage is only Pi's public materialized view. The authenticated
 * native API remains authoritative for shared records. Local-only runs use memory.
 */
export async function openDurableStorage(options: DurableStorageOptions): Promise<Storage> {
  const memory = new MemoryStorage();
  if (options.localOnly) return memory;
  const fetcher = options.fetch ?? fetch;
  const base = `/_konae/projects/${encodeURIComponent(options.projectId)}/conversations/${encodeURIComponent(options.conversationId)}`;
  let closed = false;
  let stopRequested = false;
  let generation: number | null = null;
  let leaseTimer: ReturnType<typeof setInterval> | null = null;
  let hydratedSeq = 0;
  const nativeSnapshot: NativeBatch[] = [];
  let tail = Promise.resolve();
  const uploadedBlobs = new Map<string, Promise<string>>();
  const headers = () => {
    const value: Record<string, string> = { "content-type": "application/json" };
    if (options.ownerId) value["x-konae-owner-id"] = options.ownerId;
    return value;
  };
  const request = async (url: string, init: RequestInit = {}) => {
    const response = await fetcher(url, { credentials: "same-origin", cache: "no-store", ...init, headers: { ...headers(), ...(init.headers as Record<string, string> | undefined) } });
    if (!response.ok) {
      const body = await response.text().catch(() => "");
      throw new HttpFailure(response.status, body, body ? `Conversation storage refused (${response.status}): ${body}` : `Conversation storage refused (${response.status}).`);
    }
    return response;
  };

  // Read pages in sequence order and feed them through the public MemoryStorage
  // checker. Never decode, redact, or rewrite native records in this transport.
  if (options.nativeSnapshot) {
    for (const batch of options.nativeSnapshot) {
      memory.prepareCommit(batch.writes, batch.sequence as never).apply();
      nativeSnapshot.push(batch);
      hydratedSeq = batch.sequence;
    }
  } else {
    let after = 0;
    for (;;) {
      const page = await (await request(`${base}/batches?after=${after}&limit=250&maxBytes=786432`, { method: "GET" })).json() as { batches: NativeBatch[]; nextCursor: number | null };
      for (const batch of page.batches) {
        memory.prepareCommit(batch.writes, batch.sequence as never).apply();
        nativeSnapshot.push(batch);
        hydratedSeq = batch.sequence;
      }
      if (!page.batches.length || page.nextCursor === null || page.nextCursor === after) break;
      after = page.nextCursor;
    }
  }

  return new Proxy(memory, {
    get(target, property) {
      if (property === "claimWriter") return async (expectedGeneration: number, executorId = options.executorId, continueExplicitly = false) => {
        if (!executorId || !continueExplicitly) throw new Error("Explicit Continue and a stable executor ID are required to start shared work.");
        const response = await request(`${base}/claim`, { method: "POST", body: JSON.stringify({ expectedGeneration, executorId, continueExplicitly }) });
        const value = await response.json() as { generation: number };
        const boundary = await readLastSeq();
        if (boundary !== hydratedSeq) {
          closed = true;
          await target.close(BACKGROUND_CONTEXT);
          throw new Error("Conversation history changed while claiming. Reload the conversation before continuing.");
        }
        generation = value.generation;
        leaseTimer = setInterval(() => {
          if (closed || generation === null) return;
          void request(`${base}/renew`, { method: "POST", body: JSON.stringify({ generation, executorId }) }).then(async (renewal) => {
            const state = await renewal.json() as { stopRequested?: boolean };
            if (state.stopRequested) await loseLease(new Error("Another device requested a cooperative stop. The current turn is draining before release."), true);
          }).catch((error) => loseLease(error));
        }, 15_000);
        return value.generation;
      };
      if (property === "assertLease") return async () => {
        if (closed || stopRequested || generation === null || !options.executorId) throw new Error("Shared project execution is no longer active.");
        const renewal = await request(`${base}/renew`, { method: "POST", body: JSON.stringify({ generation, executorId: options.executorId }) });
        const state = await renewal.json() as { stopRequested?: boolean };
        if (state.stopRequested) {
          await loseLease(new Error("Another device requested a cooperative stop. The current turn is draining before release."), true);
          throw new Error("A cooperative stop was requested for this project.");
        }
      };
      if (property === "commit") return (batches: readonly (readonly StorageWrite[])[], receive: (index: number, seq: never) => void, _context: Context) => ordered(async () => {
        if (closed) throw new Error("Conversation storage is closed.");
        if (generation === null) throw new Error("Conversation writer has not been claimed.");
        let stored = 0;
        const acknowledgements: number[] = [];
        const acknowledge = () => acknowledgements.forEach((seq, index) => receive(index, seq as never));
        try { for (let index = 0; index < batches.length; index += 1) {
          let prepared;
          try { prepared = target.prepareCommit(batches[index], (hydratedSeq + 1) as never); }
          catch (refused) { acknowledge(); return { stored, refused }; }
          const requestId = crypto.randomUUID();
          const draft = await options.checkpoint?.(batches[index], prepared.seq);
          const checkpoint = draft ? await prepareAtomicCheckpoint(options, draft, uploadedBlobs) : undefined;
          const exactBody = JSON.stringify({ requestId, expectedSeq: prepared.seq, generation, writes: [prepared.writes], ...(checkpoint ? { checkpoint } : {}) });
          let receipt: NativeReceipt;
          try { receipt = await sendCommit(exactBody, requestId); }
          catch (failure) {
            if (failure instanceof HttpFailure && failure.status === 422) {
              // Keep the backend's validation/refusal wording intact.
              acknowledge();
              return { stored, refused: new Error(refusalMessage(failure)) };
            }
            if (failure instanceof HttpFailure && failure.status < 500) {
              closed = true;
              await target.close(BACKGROUND_CONTEXT);
              throw failure;
            }
            try { receipt = await readReceipt(requestId); }
            catch (missing) {
              if (!(missing instanceof MissingReceipt)) {
                closed = true;
                await target.close(BACKGROUND_CONTEXT);
                throw missing;
              }
              try { receipt = await sendCommit(exactBody, requestId); }
              catch (retryFailure) {
                if (retryFailure instanceof HttpFailure && retryFailure.status === 422) {
                  acknowledge();
                  return { stored, refused: new Error(refusalMessage(retryFailure)) };
                }
                if (retryFailure instanceof HttpFailure && retryFailure.status < 500) {
                  closed = true;
                  await target.close(BACKGROUND_CONTEXT);
                  throw retryFailure;
                }
                try { receipt = await readReceipt(requestId); }
                catch { closed = true; await target.close(BACKGROUND_CONTEXT); throw retryFailure; }
              }
            }
          }
          if (receipt.requestId !== requestId || receipt.stored !== 1 || receipt.sequences.length !== 1 || receipt.sequences[0] !== prepared.seq || receipt.lastSeq !== prepared.seq || (checkpoint !== undefined && receipt.checkpointStored !== true)) {
            closed = true;
            await target.close(BACKGROUND_CONTEXT);
            throw new Error("Native conversation receipt does not match the committed Pi batch.");
          }
          prepared.apply();
          hydratedSeq = prepared.seq;
          options.onAcknowledged?.(prepared.seq);
          acknowledgements.push(prepared.seq);
          stored += 1;
        }
        } catch (error) { acknowledge(); throw error; }
        acknowledge();
        return { stored };
      });
      if (property === "mintId") return () => ordered(async () => {
        if (closed) throw new Error("Conversation storage is closed.");
        if (generation === null) throw new Error("Conversation writer has not been claimed.");
        const candidate = await target.mintId();
        const response = await request(`${base}/ids`, { method: "POST", body: JSON.stringify({ generation, candidate }) });
        return (await response.json() as { id: number }).id as never;
      });
      if (property === "readCheckpoints") return async () => readDurableCheckpoints(options, fetcher);
      if (property === "hydratedSequence") return hydratedSeq;
      if (property === "nativeSnapshot") return nativeSnapshot;
      if (property === "lastSequence") return async () => readLastSeq();
      if (property === "close") return async (context: Context = BACKGROUND_CONTEXT) => {
        if (closed) return;
        closed = true;
        if (leaseTimer) clearInterval(leaseTimer);
        leaseTimer = null;
        const value = Reflect.get(target, property, target);
        if (typeof value === "function") await value.call(target, context);
        if (generation !== null && options.executorId) {
          await request(`${base}/release`, { method: "POST", body: JSON.stringify({ generation, executorId: options.executorId }) }).catch(() => undefined);
        }
      };
      const value = Reflect.get(target, property, target);
      return typeof value === "function" ? (...args: unknown[]) => ordered(() => Promise.resolve(Reflect.apply(value, target, args))) : value;
    },
  }) as Storage;

  async function loseLease(error: unknown, cooperative = false) {
    if (closed || stopRequested) return;
    if (cooperative) stopRequested = true;
    else closed = true;
    if (leaseTimer) clearInterval(leaseTimer);
    leaseTimer = null;
    if (!cooperative) await memory.close(BACKGROUND_CONTEXT).catch(() => undefined);
    options.onLeaseLost?.(error);
  }

  function ordered<T>(run: () => Promise<T>): Promise<T> {
    const result = tail.then(run);
    tail = result.then(() => undefined, () => undefined);
    return result;
  }
  async function sendCommit(body: string, requestId: string): Promise<NativeReceipt> {
    const receipt = await (await request(`${base}/commit`, { method: "POST", body })).json() as NativeReceipt;
    if (receipt.requestId !== requestId) throw new Error("Native conversation receipt returned a different request ID.");
    return receipt;
  }
  async function readReceipt(requestId: string): Promise<NativeReceipt> {
    const response = await fetcher(`${base}/receipts/${encodeURIComponent(requestId)}`, { credentials: "same-origin", cache: "no-store", headers: headers() });
    if (response.status === 404) throw new MissingReceipt();
    if (!response.ok) throw new HttpFailure(response.status, "", `Could not read conversation receipt (${response.status}).`);
    return await response.json() as NativeReceipt;
  }
  async function fetchAllBatches() {
    const all: NativeBatch[] = [];
    let cursor = 0;
    for (;;) {
      const page = await (await request(`${base}/batches?after=${cursor}&limit=250&maxBytes=786432`, { method: "GET" })).json() as { batches: NativeBatch[]; nextCursor: number | null };
      all.push(...page.batches);
      if (!page.batches.length || page.nextCursor === null || page.nextCursor === cursor) return all;
      cursor = page.nextCursor;
    }
  }
  async function readLastSeq() {
    const info = await (await request(base, { method: "GET" })).json() as DurableConversationInfo;
    return info.lastSeq;
  }
}

async function prepareAtomicCheckpoint(options: DurableStorageOptions, draft: DurableCheckpointDraft, cache: Map<string, Promise<string>>) {
  const blobs: Record<string, string> = Object.create(null);
  for (const blob of draft.blobs ?? []) {
    const digest = await sha256(blob.bytes);
    let uploaded = cache.get(digest);
    if (!uploaded) { uploaded = uploadDurableBlob(options, blob.bytes, options.fetch ?? fetch, digest); cache.set(digest, uploaded); }
    blobs[blob.key] = await uploaded;
  }
  return { id: draft.id ?? crypto.randomUUID(), payload: { ...draft.payload, blobFiles: { ...(draft.payload.blobFiles as Record<string, string> | undefined), ...blobs }, blobs: [...new Set([...(Array.isArray(draft.payload.blobs) ? draft.payload.blobs as string[] : []), ...Object.values(blobs)])] } };
}

export async function uploadDurableBlob(options: Pick<DurableStorageOptions, "projectId" | "conversationId" | "ownerId">, bytes: Uint8Array, fetcher: typeof fetch = fetch, knownDigest?: string) {
  const digest = knownDigest ?? await sha256(bytes);
  const headers: Record<string, string> = { "content-type": "application/octet-stream" };
  if (options.ownerId) headers["x-konae-owner-id"] = options.ownerId;
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(options.projectId)}/conversations/${encodeURIComponent(options.conversationId)}/blobs/${digest}`, { method: "PUT", credentials: "same-origin", cache: "no-store", headers, body: bytes.slice().buffer as ArrayBuffer });
  if (!response.ok) throw new Error(`Could not store immutable conversation file (${response.status}).`);
  return digest;
}

export async function readDurableBlob(options: Pick<DurableStorageOptions, "projectId" | "conversationId" | "ownerId">, digest: string, fetcher: typeof fetch = fetch) {
  if (!/^[a-f0-9]{64}$/i.test(digest)) throw new Error("Invalid immutable file digest.");
  const headers = options.ownerId ? { "x-konae-owner-id": options.ownerId } : undefined;
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(options.projectId)}/conversations/${encodeURIComponent(options.conversationId)}/blobs/${digest}`, { credentials: "same-origin", cache: "no-store", headers });
  if (!response.ok) throw new Error(`Could not read immutable conversation file (${response.status}).`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (await sha256(bytes) !== digest.toLowerCase()) throw new Error("Immutable conversation file digest does not match its content.");
  return bytes;
}

export async function readDurableCheckpoints(options: Pick<DurableStorageOptions, "projectId" | "conversationId" | "ownerId">, fetcher: typeof fetch = fetch): Promise<DurableCheckpoint[]> {
  const headers = options.ownerId ? { "x-konae-owner-id": options.ownerId } : undefined;
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(options.projectId)}/conversations/${encodeURIComponent(options.conversationId)}/checkpoints`, { credentials: "same-origin", cache: "no-store", headers });
  if (!response.ok) throw new Error(`Could not read conversation checkpoints (${response.status}).`);
  const page = await response.json() as { items: Array<{ checkpointId: string; sequence: number; payload: Record<string, unknown>; createdAt: number }> };
  return page.items;
}

async function readCheckpointSelection(options: Pick<DurableStorageOptions, "projectId" | "conversationId" | "ownerId">, query: URLSearchParams, fetcher: typeof fetch) {
  const headers = options.ownerId ? { "x-konae-owner-id": options.ownerId } : undefined;
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(options.projectId)}/conversations/${encodeURIComponent(options.conversationId)}/checkpoints?${query}`, { credentials: "same-origin", cache: "no-store", headers });
  if (!response.ok) throw new Error(`Could not read the selected proposal checkpoint (${response.status}).`);
  return (await response.json() as { items: DurableCheckpoint[] }).items[0] ?? null;
}

/** Reads only the latest checkpoint metadata and files; preserves the owner route and sequence fence. */
export function readLatestDurableCheckpoint(options: Pick<DurableStorageOptions, "projectId" | "conversationId" | "ownerId">, fetcher: typeof fetch = fetch) {
  return readCheckpointSelection(options, new URLSearchParams({ latest: "true" }), fetcher);
}

/** Reads one immutable checkpoint by owner-scoped ID. */
export function readDurableCheckpoint(options: Pick<DurableStorageOptions, "projectId" | "conversationId" | "ownerId">, checkpointId: string, fetcher: typeof fetch = fetch) {
  return readCheckpointSelection(options, new URLSearchParams({ checkpoint_id: checkpointId }), fetcher);
}

async function sha256(bytes: Uint8Array) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes.slice().buffer as ArrayBuffer));
  return [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

class MissingReceipt extends Error {}
class HttpFailure extends Error {
  constructor(readonly status: number, readonly body: string, message: string) { super(message); }
}

function refusalMessage(failure: HttpFailure) {
  try {
    const parsed = JSON.parse(failure.body) as { error?: unknown };
    if (typeof parsed.error === "string") return parsed.error;
  } catch { /* the native refusal body may already be a plain sentence */ }
  return failure.body || failure.message;
}

export async function createDurableConversation(projectId: string, ownerId?: string, fetcher: typeof fetch = fetch, title = "New conversation", id: string = crypto.randomUUID()): Promise<DurableConversationInfo> {
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations`, {
    method: "POST", credentials: "same-origin", cache: "no-store",
    headers: { "content-type": "application/json", ...(ownerId ? { "x-konae-owner-id": ownerId } : {}) },
    body: JSON.stringify({ id, formatVersion: 1, piVersion: "1.5.2", registryVersion: "1", title }),
  });
  if (!response.ok) throw new Error(`Could not create conversation (${response.status}).`);
  return await response.json() as DurableConversationInfo;
}

export type LocalHistoryAttachment = {
  version: 1;
  attachmentId: string;
  recordIdMap: Record<string, number>;
  createdAt: number;
  title: string;
  records: Array<Record<string, any>>;
  modelContext: { messages: unknown[] };
  files: { current: Record<string, string>; encodings?: Record<string, "base64">; candidate?: Record<string, string>; candidateEncodings?: Record<string, "base64">; changedFiles?: string[] };
  continuation: { eligible: boolean; reasons: string[] };
};

/** Explicitly copy a local transcript and its proposal checkpoint into an owned Project. Never saves Project sources. */
export async function attachLocalHistory(options: {
  project: { id: string; revision: number; files: Record<string, string>; encodings?: Record<string, "base64">; modules?: Record<string, unknown> };
  ownerId: string;
  attachment: LocalHistoryAttachment;
  fetch?: typeof fetch;
}): Promise<{ conversation: DurableConversationInfo; copiedEntries: number; checkpointStored: boolean; inspectionOnly: boolean }> {
  const fetcher = options.fetch ?? fetch;
  const title = `Local attachment · ${options.attachment.title}`.slice(0, 120);
  let conversation = await readDurableConversation(options.project.id, options.attachment.attachmentId, options.ownerId, fetcher).catch(() => null);
  if (!conversation) conversation = await createDurableConversation(options.project.id, options.ownerId, fetcher, title, options.attachment.attachmentId);
  else {
    let saved = await readDurableRecordIds(options.project.id, conversation.id, options.ownerId, undefined, fetcher);
    const savedIds = new Set<string>();
    for (;;) {
      for (const item of saved.items) savedIds.add(String(item.id));
      if (!saved.nextCursor) break;
      saved = await readDurableRecordIds(options.project.id, conversation.id, options.ownerId, saved.nextCursor, fetcher);
    }
    const expectedIds = options.attachment.records.map((entry) => String(options.attachment.recordIdMap[String(entry.id)]));
    if (expectedIds.every((id) => savedIds.has(id)) && savedIds.size === expectedIds.length) {
      const expectedLastSeq = expectedIds.length ? 2 : 1;
      const summary = await readDurableCheckpointSummary(options.project.id, conversation.id, options.ownerId, fetcher);
      if (conversation.lastSeq !== expectedLastSeq || !summary.item || summary.item.sequence !== expectedLastSeq) throw new Error("The prior attachment's stable ID is present, but its checkpoint/boundary read-back is incomplete. Inspect it before retrying.");
      return { conversation, copiedEntries: expectedIds.length, checkpointStored: true, inspectionOnly: !options.attachment.continuation.eligible };
    }
    if (expectedIds.some((id) => savedIds.has(id))) throw new Error("The prior attachment has a partial native entry set. No second copy was written; inspect this stable attachment before retrying.");
  }
  const executorId = `local-attachment-${crypto.randomUUID()}`;
  let storage: Storage | null = null;
  let checkpointStored = false;
  try {
    const lease = await readDurableLease(options.project.id, conversation.id, options.ownerId, fetcher);
    if (lease.live) throw new Error("Another browser is actively writing this Project. Read its status and finish the handoff before attaching local history.");
    if (lease.unresolvedEffects) throw new Error("An admitted or unknown write is unresolved. Reconcile it before attaching local history.");
    const eligible = options.attachment.continuation.eligible;
    storage = await openDurableStorage({ projectId: options.project.id, conversationId: conversation.id, ownerId: options.ownerId, executorId, fetch: fetcher,
      checkpoint: async (writes, sequence) => {
        if (!writes.some((write) => write.type === "entry" || write.type === "conversation")) return null;
        const { createCheckpointDraft } = await import("./durable-checkpoints");
        const candidateFiles = options.attachment.files.candidate ?? options.attachment.files.current;
        const hasProposal = JSON.stringify(candidateFiles) !== JSON.stringify(options.project.files);
        return createCheckpointDraft({
          projectId: options.project.id, sourcesRevision: options.project.revision,
          base: { files: options.project.files, encodings: options.project.encodings },
          scratch: { files: candidateFiles, encodings: options.attachment.files.candidateEncodings ?? options.attachment.files.encodings }, modelView: { files: candidateFiles, encodings: options.attachment.files.candidateEncodings ?? options.attachment.files.encodings },
          modelContext: { sequence, messages: options.attachment.modelContext.messages }, nativeSequence: sequence,
          validationSettings: { modules: options.project.modules ?? {} }, status: hasProposal ? "proposal" : "working",
          ...(hasProposal ? { candidateId: crypto.randomUUID() } : {}),
          lifecycle: { origin: "local-attachment", inspectionOnly: !eligible, reasons: options.attachment.continuation.reasons },
        });
      },
    });
    const generation = await readDurableLease(options.project.id, conversation.id, options.ownerId, fetcher);
    if (generation.live || generation.unresolvedEffects) throw new Error("Project execution changed before local attachment. No local provider or source operation was started.");
    await (storage as any).claimWriter(generation.generation, executorId, true);
    const hasRoot = conversation.lastSeq > 0;
    if (!hasRoot) {
      const root = { type: "conversation", value: { id: ROOT_CONVERSATION_ID } } as StorageWrite;
      const rootReceipt = await storage.commit([[root]], () => {}, BACKGROUND_CONTEXT);
      if (rootReceipt.stored !== 1 || rootReceipt.refused) throw new Error(`Local attachment root was not acknowledged: ${String(rootReceipt.refused ?? "unknown refusal")}`);
      checkpointStored = true;
    }

    const ids = new Map(options.attachment.records.map((entry) => [String(entry.id), options.attachment.recordIdMap[String(entry.id)]]));
    if (ids.size !== options.attachment.records.length || [...ids.values()].some((id) => !Number.isSafeInteger(id) || id <= ROOT_CONVERSATION_ID)) throw new Error("The stable attachment ID map is incomplete or invalid.");
    const entries = options.attachment.records.map((entry) => {
      const value: Record<string, unknown> = { ...entry, id: ids.get(String(entry.id)), conversationId: ROOT_CONVERSATION_ID };
      delete value.byTaskId;
      if (typeof entry.head === "number") value.head = ids.get(String(entry.head));
      if (Array.isArray(entry.edits)) value.edits = entry.edits.flatMap((edit: Record<string, unknown>) => {
        const target = ids.get(String(edit.target));
        return target === undefined ? [] : [{ ...edit, target }];
      });
      return { type: "entry", value } as StorageWrite;
    });
    if (entries.length) {
      const receipt = await storage.commit([entries], () => {}, BACKGROUND_CONTEXT);
      if (receipt.stored !== 1 || receipt.refused) throw new Error(`Local history entries were not acknowledged: ${String(receipt.refused ?? "unknown refusal")}`);
      checkpointStored = true;
    }
    await storage.close(BACKGROUND_CONTEXT);
    storage = null;
    const observed = await readDurableConversation(options.project.id, conversation.id, options.ownerId, fetcher);
    const expectedLastSeq = entries.length ? 2 : 1;
    if (observed.lastSeq !== expectedLastSeq) throw new Error(`Local attachment read-back expected native sequence ${expectedLastSeq}, but saw ${observed.lastSeq}.`);
    let readBack = await readDurableRecordIds(options.project.id, conversation.id, options.ownerId, undefined, fetcher);
    const observedIds = new Set<number>();
    for (;;) {
      for (const item of readBack.items) if (item.id !== null) observedIds.add(item.id);
      if (!readBack.nextCursor) break;
      readBack = await readDurableRecordIds(options.project.id, conversation.id, options.ownerId, readBack.nextCursor, fetcher);
    }
    if (!entries.every((entry) => entry.type === "entry" && observedIds.has(Number(entry.value.id)))) throw new Error("The owner batch read-back did not confirm every attached native entry. Keep this stable handoff and reconcile before retrying.");
    if (checkpointStored) {
      const summary = await readDurableCheckpointSummary(options.project.id, conversation.id, options.ownerId, fetcher);
      checkpointStored = summary.item?.sequence === expectedLastSeq;
      if (!checkpointStored) throw new Error("The local attachment's proposal checkpoint was not visible after commit.");
    }
    return { conversation: observed, copiedEntries: entries.length, checkpointStored, inspectionOnly: !eligible };
  } catch (error) {
    if (storage) await storage.close(BACKGROUND_CONTEXT).catch(() => undefined);
    throw error;
  }
}

export async function readDurableConversation(projectId: string, conversationId: string, ownerId?: string, fetcher: typeof fetch = fetch) {
  const headers = ownerId ? { "x-konae-owner-id": ownerId } : undefined;
  const base = `/_konae/projects/${encodeURIComponent(projectId)}/conversations/${encodeURIComponent(conversationId)}`;
  const response = await fetcher(base, { credentials: "same-origin", cache: "no-store", headers });
  if (!response.ok) throw new Error(`Could not read conversation (${response.status}).`);
  return await response.json() as DurableConversationInfo;
}


/** Store private result detail out of first-page native records; callers keep the exact model text separately. */
export async function uploadDurableDetail(options: DurableStorageOptions, value: unknown, fetcher: typeof fetch = fetch) {
  const bytes = new TextEncoder().encode(JSON.stringify(value));
  if (bytes.byteLength > 1024 * 1024) throw new Error("Structured tool detail exceeds the 1 MiB conversation detail limit.");
  const digestBytes = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  const digest = [...digestBytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
  const base = `/_konae/projects/${encodeURIComponent(options.projectId)}/conversations/${encodeURIComponent(options.conversationId)}/blobs/${digest}`;
  const headers: Record<string, string> = { "content-type": "application/octet-stream" };
  if (options.ownerId) headers["x-konae-owner-id"] = options.ownerId;
  const response = await fetcher(base, { method: "PUT", credentials: "same-origin", cache: "no-store", headers, body: bytes });
  if (!response.ok) throw new Error(`Could not store structured tool detail (${response.status}).`);
  return { kind: "immutable-detail", digest, byteLength: bytes.byteLength } as const;
}

export async function readDurableDetail<T = unknown>(options: DurableStorageOptions, digest: string, fetcher: typeof fetch = fetch): Promise<T> {
  if (!/^[a-f0-9]{64}$/i.test(digest)) throw new Error("Invalid immutable detail digest.");
  const headers = options.ownerId ? { "x-konae-owner-id": options.ownerId } : undefined;
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(options.projectId)}/conversations/${encodeURIComponent(options.conversationId)}/blobs/${digest}`, { credentials: "same-origin", cache: "no-store", headers });
  if (!response.ok) throw new Error(`Could not read structured tool detail (${response.status}).`);
  return await response.json() as T;
}
