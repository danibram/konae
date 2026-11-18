import { describe, expect, test } from "bun:test";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { StorageWrite } from "@earendil-works/pi-durable";
import { ROOT_CONVERSATION_ID } from "@earendil-works/pi-durable";
import { CompactionEntry } from "@earendil-works/pi-durable";
import { registerStorageConformance } from "@earendil-works/pi-durable/testing";
import { openDurableStorage, readDurableBlob, requestDurableStop } from "./durable-storage";

function nativeApi(options: { loseAck?: boolean; refuse?: string; changeAfterClaim?: boolean } = {}) {
  const batches: Array<{ sequence: number; writes: StorageWrite[] }> = [];
  const checkpoints: Array<{ checkpointId: string; sequence: number; payload: Record<string, unknown>; createdAt: number }> = [];
  const blobs = new Map<string, Uint8Array>();
  const receipts = new Map<string, unknown>();
  const requests: Array<{ url: string; method: string; body?: string; headers: Headers }> = [];
  let generation = 0;
  let leaseLive = false;
  let stopRequested = false;
  let highWater = 0;
  let loseAck = options.loseAck ?? false;
  let refuse = options.refuse;
  const fetcher: typeof fetch = async (input, init = {}) => {
    const url = String(input);
    const method = init.method ?? "GET";
    const body = typeof init.body === "string" ? init.body : undefined;
    const headers = new Headers(init.headers);
    requests.push({ url, method, body, headers });
    if (/\/conversations\/[^/]+$/.test(new URL(url, "http://local").pathname)) return Response.json({ lastSeq: batches.at(-1)?.sequence ?? 0 });
    if (url.includes("/batches?")) return Response.json({ batches: [...batches].filter((batch) => batch.sequence > Number(new URL(url, "http://local").searchParams.get("after"))).sort((a, b) => a.sequence - b.sequence), nextCursor: batches.at(-1)?.sequence ?? null });
    if (url.endsWith("/claim")) {
      const claim = JSON.parse(body!) as { executorId?: string; continueExplicitly?: boolean };
      if (!claim.executorId || !claim.continueExplicitly || leaseLive) return Response.json({ error: "lease held or explicit Continue missing" }, { status: 409 });
      generation += 1;
      leaseLive = true;
      if (options.changeAfterClaim && batches.length === 0) batches.push({ sequence: 1, writes: [{ type: "conversation", value: { id: 1 } } as StorageWrite] });
      return Response.json({ generation, claimedAt: 1, expiresAt: 60_001 });
    }
    if (url.endsWith("/renew")) return leaseLive ? Response.json({ generation, expiresAt: 60_001, stopRequested }) : Response.json({ error: "expired" }, { status: 409 });
    if (url.endsWith("/stop")) { stopRequested = true; return Response.json({ requested: true, requestId: JSON.parse(body!).requestId, executorId: "fixture-executor", generation }); }
    if (url.endsWith("/release")) { leaseLive = false; return Response.json({ generation, released: true }); }
    if (url.endsWith("/commit")) {
      if (refuse) return Response.json({ error: refuse }, { status: 422 });
      const value = JSON.parse(body!) as { requestId: string; expectedSeq: number; writes: StorageWrite[][] };
      const sequences = value.writes.map((writes, index) => {
        const sequence = value.expectedSeq + index;
        batches.push({ sequence, writes });
        return sequence;
      });
      if (value.checkpoint) checkpoints.push({ checkpointId: value.checkpoint.id, sequence: sequences.at(-1)!, payload: value.checkpoint.payload, createdAt: 1 });
      const receipt = { requestId: value.requestId, stored: sequences.length, sequences, lastSeq: sequences.at(-1), refused: null, checkpointStored: Boolean(value.checkpoint) };
      receipts.set(value.requestId, receipt);
      if (loseAck) { loseAck = false; throw new TypeError("connection reset after server commit"); }
      return Response.json(receipt);
    }
    const receiptMatch = url.match(/\/receipts\/([^/?]+)/);
    if (receiptMatch) {
      const receipt = receipts.get(decodeURIComponent(receiptMatch[1]));
      return receipt ? Response.json(receipt) : Response.json({ error: "missing" }, { status: 404 });
    }
    if (url.endsWith("/ids")) {
      const value = JSON.parse(body!) as { candidate: number };
      highWater = Math.max(highWater + 1, value.candidate);
      return Response.json({ id: highWater });
    }
    if (url.endsWith("/checkpoints")) return Response.json({ items: checkpoints });
    const blobMatch = url.match(/\/blobs\/([a-f0-9]{64})$/);
    if (blobMatch && method === "PUT") {
      const bytes = new Uint8Array(await new Response(init.body).arrayBuffer());
      blobs.set(blobMatch[1], bytes);
      return Response.json({ stored: true });
    }
    if (blobMatch && method === "GET") {
      const bytes = blobs.get(blobMatch[1]);
      return bytes ? new Response(bytes) : Response.json({ error: "missing" }, { status: 404 });
    }
    return Response.json({ error: "unexpected route" }, { status: 404 });
  };
  return { fetcher, requests, batches, checkpoints, blobs, setRefuse(value: string | undefined) { refuse = value; } };
}

const initialConversation = [{ type: "conversation", value: { id: ROOT_CONVERSATION_ID } }] as unknown as StorageWrite[];

describe("authenticated native Durable Storage bridge", () => {
  test("requires explicit Continue, renews with the stable executor, and cooperatively releases", async () => {
    const api = nativeApi();
    const storage = await openDurableStorage({ projectId: "lease-project", conversationId: "lease-conversation", executorId: "stable-device", fetch: api.fetcher });
    await expect((storage as any).claimWriter(0)).rejects.toThrow("Explicit Continue");
    await (storage as any).claimWriter(0, "stable-device", true);
    await (storage as any).assertLease();
    expect(api.requests.filter(({ url }) => url.endsWith("/renew"))).toHaveLength(1);
    await storage.close(BACKGROUND_CONTEXT);
    expect(api.requests.find(({ url }) => url.endsWith("/release"))?.body).toContain("stable-device");
  });

  test("a project-scoped stop request fences further provider admission and leaves release to drain", async () => {
    const api = nativeApi();
    const storage = await openDurableStorage({ projectId: "stop-project", conversationId: "stop-conversation", executorId: "stable-device", fetch: api.fetcher });
    await (storage as any).claimWriter(0, "stable-device", true);
    const stop = await requestDurableStop("stop-project", "stop-conversation", 1, "stop-request-1", undefined, api.fetcher);
    expect(stop).toMatchObject({ requested: true, requestId: "stop-request-1" });
    await expect((storage as any).assertLease()).rejects.toThrow("cooperative stop");
    expect(api.requests.some(({ url }) => url.endsWith("/release"))).toBe(false);
    await storage.close(BACKGROUND_CONTEXT);
    expect(api.requests.some(({ url }) => url.endsWith("/release"))).toBe(true);
  });

  test("reuses the exact request bytes after a lost acknowledgement and reads the receipt", async () => {
    const api = nativeApi({ loseAck: true });
    const acknowledged: number[] = [];
    const storage = await openDurableStorage({ projectId: "project-a", conversationId: "conversation-a", ownerId: "owner-a", fetch: api.fetcher, onAcknowledged: (sequence) => acknowledged.push(sequence) });
    const claimed = await (storage as any).claimWriter(0, "fixture-executor", true);
    expect(claimed).toBe(1);
    const seen: number[] = [];
    const result = await storage.commit([initialConversation], (_index, seq) => seen.push(seq), BACKGROUND_CONTEXT);
    expect(result.stored).toBe(1);
    expect(seen).toEqual([1]);
    expect(acknowledged).toEqual([1]);
    expect((storage as any).hydratedSequence).toBe(1);
    expect(await (storage as any).lastSequence()).toBe(1);
    expect(api.requests.filter((request) => /\/conversations\/[^/]+$/.test(new URL(request.url, "http://local").pathname))).toHaveLength(2);
    const post = api.requests.find((request) => request.url.endsWith("/commit"))!;
    expect(post.headers.get("x-konae-owner-id")).toBe("owner-a");
    expect(post.body).toContain('"requestId"');
    expect(api.requests.filter((request) => request.url.includes("/receipts/")).length).toBe(1);
    expect((await storage.conversation(ROOT_CONVERSATION_ID, BACKGROUND_CONTEXT))?.id).toBe(ROOT_CONVERSATION_ID);
  });

  test("does not expose optimistic records after a definitive backend refusal", async () => {
    const api = nativeApi({ refuse: "A Pi entry ID already exists." });
    const storage = await openDurableStorage({ projectId: "p", conversationId: "c", fetch: api.fetcher });
    await (storage as any).claimWriter(0, "fixture-executor", true);
    const result = await storage.commit([initialConversation], () => {}, BACKGROUND_CONTEXT);
    expect(result.stored).toBe(0);
    expect((result.refused as Error).message).toContain("A Pi entry ID already exists.");
    expect(await storage.conversation(ROOT_CONVERSATION_ID, BACKGROUND_CONTEXT)).toBeUndefined();
    expect(api.requests.filter((request) => request.url.endsWith("/commit"))).toHaveLength(1);
  });

  test("refuses writer activation when the history advances after claim", async () => {
    const api = nativeApi({ changeAfterClaim: true });
    const storage = await openDurableStorage({ projectId: "p", conversationId: "c", fetch: api.fetcher });
    await expect((storage as any).claimWriter(0, "fixture-executor", true)).rejects.toThrow("Conversation history changed while claiming");
  });

  test("round-trips compaction summary, head provenance and reason through the authenticated batch log", async () => {
    const api = nativeApi();
    const storage = await openDurableStorage({ projectId: "p", conversationId: "c", fetch: api.fetcher });
    await (storage as any).claimWriter(0, "fixture-executor", true);
    const root = [{ type: "conversation", value: { id: ROOT_CONVERSATION_ID } }] as unknown as StorageWrite[];
    await storage.commit([root], () => {}, BACKGROUND_CONTEXT);
    const summary = "The owner asked for a checked preview. The last retained entry is the new source of truth.";
    const entries = [
      { id: 2, conversationId: ROOT_CONVERSATION_ID, kind: "pi.user", model: [{ role: "user", content: "Earlier owner request", timestamp: 10 }] },
      { id: 3, conversationId: ROOT_CONVERSATION_ID, kind: CompactionEntry.kind, head: 2, model: [{ role: "user", content: `Compaction summary: ${summary}`, timestamp: 11 }], data: { reason: "manual" } },
    ];
    await storage.commit([entries.map((value) => ({ type: "entry", value })) as unknown as StorageWrite[]], () => {}, BACKGROUND_CONTEXT);
    const stored = await storage.entry(ROOT_CONVERSATION_ID, 3 as never, BACKGROUND_CONTEXT);
    const compaction = stored?.entry;
    expect(CompactionEntry.is(compaction)).toBe(true);
    expect(compaction).toMatchObject({ id: 3, conversationId: ROOT_CONVERSATION_ID, head: 2, model: [{ role: "user", content: `Compaction summary: ${summary}`, timestamp: 11 }], data: { reason: "manual" } });
    expect(api.batches[1]?.writes[1]).toMatchObject({ type: "entry", value: { kind: "pi.compaction", head: 2, data: { reason: "manual" } } });
    await storage.close(BACKGROUND_CONTEXT);
  });

  test("commits immutable binary checkpoint metadata in the same native transaction as its file result", async () => {
    const api = nativeApi({ loseAck: true });
    const storage = await openDurableStorage({
      projectId: "p", conversationId: "c", fetch: api.fetcher,
      checkpoint: async (_writes, seq) => ({
        id: "checkpoint-file-result",
        payload: { formatVersion: 1, sequence: seq, status: "working", blobFiles: { "scratch:public/pixel.png": "" }, blobs: [],
          context: { messages: [{ role: "user", content: "add an image" }] }, sourcesRevision: 4 },
        blobs: [{ key: "scratch:public/pixel.png", bytes: new Uint8Array([0, 255, 1, 128]) }],
      }),
    });
    await (storage as any).claimWriter(0, "fixture-executor", true);
    const result = await storage.commit([initialConversation], () => {}, BACKGROUND_CONTEXT);
    expect(result.stored).toBe(1);
    expect(api.batches).toHaveLength(1);
    expect(api.checkpoints).toHaveLength(1);
    expect(api.checkpoints[0]).toMatchObject({ checkpointId: "checkpoint-file-result", sequence: 1, payload: { sequence: 1, status: "working" } });
    expect(api.requests.filter((request) => request.url.endsWith("/commit"))).toHaveLength(1);
    const sent = JSON.parse(api.requests.find((request) => request.url.endsWith("/commit"))!.body!) as { checkpoint: unknown };
    expect(sent.checkpoint).toBeDefined();
    const recoveredStorage = await openDurableStorage({ projectId: "p", conversationId: "c", fetch: api.fetcher });
    const recovered = await (recoveredStorage as any).readCheckpoints();
    expect(recovered).toMatchObject([{ checkpointId: "checkpoint-file-result", sequence: 1, payload: { context: { messages: [{ role: "user", content: "add an image" }] } } }]);
    const digest = (recovered[0].payload.blobFiles as Record<string, string>)["scratch:public/pixel.png"];
    expect([...await readDurableBlob({ projectId: "p", conversationId: "c" }, digest, api.fetcher)]).toEqual([0, 255, 1, 128]);
    await recoveredStorage.close(BACKGROUND_CONTEXT);
  });

  test("a refused native batch leaves the previous checkpoint boundary visible", async () => {
    const api = nativeApi();
    const storage = await openDurableStorage({ projectId: "p", conversationId: "c", fetch: api.fetcher, checkpoint: async (_writes, sequence) => ({ id: `checkpoint-${sequence}`, payload: { formatVersion: 1, sequence }, blobs: [] }) });
    await (storage as any).claimWriter(0, "fixture-executor", true);
    await storage.commit([initialConversation], () => {}, BACKGROUND_CONTEXT);
    api.setRefuse("checkpoint batch refused");
    const result = await storage.commit([[{ type: "entry", value: { id: 2, conversationId: ROOT_CONVERSATION_ID, kind: "pi.user", model: [] } } as StorageWrite]], () => {}, BACKGROUND_CONTEXT);
    expect(result.stored).toBe(0);
    expect(api.checkpoints).toHaveLength(1);
    expect(await (storage as any).readCheckpoints()).toMatchObject([{ checkpointId: "checkpoint-1", sequence: 1 }]);
  });
});

registerStorageConformance({ describe, expect, it: test }, "authenticated native batch bridge", async (use) => {
  const api = nativeApi();
  const storage = await openDurableStorage({ projectId: "conformance-project", conversationId: "conformance-conversation", fetch: api.fetcher });
  await (storage as any).claimWriter(0, "fixture-executor", true);
  try { await use(storage); }
  finally { await storage.close(BACKGROUND_CONTEXT); }
});
