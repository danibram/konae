import type { Context } from "@earendil-works/chord/context";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { MemoryStorage } from "@earendil-works/pi-durable/storage/memory";
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
};

export type DurableStorageOptions = {
  projectId: string;
  conversationId: string;
  ownerId?: string;
  fetch?: typeof fetch;
  localOnly?: boolean;
};

type NativeReceipt = { requestId: string; stored: number; sequences: number[]; lastSeq: number; refused: string | null };
type NativeBatch = { sequence: number; writes: StorageWrite[] };

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
  let generation: number | null = null;
  let hydratedSeq = 0;
  let tail = Promise.resolve();
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
  let after = 0;
  for (;;) {
    const page = await (await request(`${base}/batches?after=${after}&limit=50`, { method: "GET" })).json() as { batches: NativeBatch[]; nextCursor: number | null };
    for (const batch of page.batches) {
      memory.prepareCommit(batch.writes, batch.sequence as never).apply();
      after = batch.sequence;
      hydratedSeq = batch.sequence;
    }
    if (page.batches.length < 50 || page.nextCursor === null || page.nextCursor === after && page.batches.length === 0) break;
  }

  return new Proxy(memory, {
    get(target, property) {
      if (property === "claimWriter") return async (expectedGeneration: number) => {
        const response = await request(`${base}/claim`, { method: "POST", body: JSON.stringify({ expectedGeneration }) });
        const value = await response.json() as { generation: number };
        const boundary = await readLastSeq();
        if (boundary !== hydratedSeq) {
          closed = true;
          await target.close(BACKGROUND_CONTEXT);
          throw new Error("Conversation history changed while claiming. Reload the conversation before continuing.");
        }
        generation = value.generation;
        return value.generation;
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
          const exactBody = JSON.stringify({ requestId, expectedSeq: prepared.seq, generation, writes: [prepared.writes] });
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
          if (receipt.requestId !== requestId || receipt.stored !== 1 || receipt.sequences.length !== 1 || receipt.sequences[0] !== prepared.seq || receipt.lastSeq !== prepared.seq) {
            closed = true;
            await target.close(BACKGROUND_CONTEXT);
            throw new Error("Native conversation receipt does not match the committed Pi batch.");
          }
          prepared.apply();
          hydratedSeq = prepared.seq;
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
      if (property === "close") return async (context: Context = BACKGROUND_CONTEXT) => {
        closed = true;
        const value = Reflect.get(target, property, target);
        if (typeof value === "function") await value.call(target, context);
      };
      const value = Reflect.get(target, property, target);
      return typeof value === "function" ? (...args: unknown[]) => ordered(() => Promise.resolve(Reflect.apply(value, target, args))) : value;
    },
  }) as Storage;

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
      const page = await (await request(`${base}/batches?after=${cursor}&limit=50`, { method: "GET" })).json() as { batches: NativeBatch[] };
      all.push(...page.batches);
      if (page.batches.length < 50) return all;
      cursor = page.batches.at(-1)!.sequence;
    }
  }
  async function readLastSeq() {
    const all = await fetchAllBatches();
    return all.at(-1)?.sequence ?? 0;
  }
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

export async function createDurableConversation(projectId: string, ownerId?: string, fetcher: typeof fetch = fetch): Promise<DurableConversationInfo> {
  const id = crypto.randomUUID();
  const response = await fetcher(`/_konae/projects/${encodeURIComponent(projectId)}/conversations`, {
    method: "POST", credentials: "same-origin", cache: "no-store",
    headers: { "content-type": "application/json", ...(ownerId ? { "x-konae-owner-id": ownerId } : {}) },
    body: JSON.stringify({ id, formatVersion: 1, piVersion: "1.5.2", registryVersion: "1", title: "New conversation" }),
  });
  if (!response.ok) throw new Error(`Could not create conversation (${response.status}).`);
  return await response.json() as DurableConversationInfo;
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
