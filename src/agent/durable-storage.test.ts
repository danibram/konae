import { describe, expect, test } from "bun:test";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { StorageWrite } from "@earendil-works/pi-durable";
import { ROOT_CONVERSATION_ID } from "@earendil-works/pi-durable";
import { CompactionEntry } from "@earendil-works/pi-durable";
import { registerStorageConformance } from "@earendil-works/pi-durable/testing";
import { openDurableStorage } from "./durable-storage";

function nativeApi(options: { loseAck?: boolean; refuse?: string; changeAfterClaim?: boolean } = {}) {
  const batches: Array<{ sequence: number; writes: StorageWrite[] }> = [];
  const receipts = new Map<string, unknown>();
  const requests: Array<{ url: string; method: string; body?: string; headers: Headers }> = [];
  let generation = 0;
  let highWater = 0;
  let loseAck = options.loseAck ?? false;
  const fetcher: typeof fetch = async (input, init = {}) => {
    const url = String(input);
    const method = init.method ?? "GET";
    const body = typeof init.body === "string" ? init.body : undefined;
    const headers = new Headers(init.headers);
    requests.push({ url, method, body, headers });
    if (url.includes("/batches?")) return Response.json({ batches: [...batches].filter((batch) => batch.sequence > Number(new URL(url, "http://local").searchParams.get("after"))).sort((a, b) => a.sequence - b.sequence), nextCursor: batches.at(-1)?.sequence ?? null });
    if (url.endsWith("/claim")) {
      generation += 1;
      if (options.changeAfterClaim && batches.length === 0) batches.push({ sequence: 1, writes: [{ type: "conversation", value: { id: 1 } } as StorageWrite] });
      return Response.json({ generation, claimedAt: 1 });
    }
    if (url.endsWith("/commit")) {
      if (options.refuse) return Response.json({ error: options.refuse }, { status: 422 });
      const value = JSON.parse(body!) as { requestId: string; expectedSeq: number; writes: StorageWrite[][] };
      const sequences = value.writes.map((writes, index) => {
        const sequence = value.expectedSeq + index;
        batches.push({ sequence, writes });
        return sequence;
      });
      const receipt = { requestId: value.requestId, stored: sequences.length, sequences, lastSeq: sequences.at(-1), refused: null };
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
    return Response.json({ error: "unexpected route" }, { status: 404 });
  };
  return { fetcher, requests, batches };
}

const initialConversation = [{ type: "conversation", value: { id: ROOT_CONVERSATION_ID } }] as unknown as StorageWrite[];

describe("authenticated native Durable Storage bridge", () => {
  test("reuses the exact request bytes after a lost acknowledgement and reads the receipt", async () => {
    const api = nativeApi({ loseAck: true });
    const storage = await openDurableStorage({ projectId: "project-a", conversationId: "conversation-a", ownerId: "owner-a", fetch: api.fetcher });
    const claimed = await (storage as any).claimWriter(0);
    expect(claimed).toBe(1);
    const seen: number[] = [];
    const result = await storage.commit([initialConversation], (_index, seq) => seen.push(seq), BACKGROUND_CONTEXT);
    expect(result.stored).toBe(1);
    expect(seen).toEqual([1]);
    const post = api.requests.find((request) => request.url.endsWith("/commit"))!;
    expect(post.headers.get("x-konae-owner-id")).toBe("owner-a");
    expect(post.body).toContain('"requestId"');
    expect(api.requests.filter((request) => request.url.includes("/receipts/")).length).toBe(1);
    expect((await storage.conversation(ROOT_CONVERSATION_ID, BACKGROUND_CONTEXT))?.id).toBe(ROOT_CONVERSATION_ID);
  });

  test("does not expose optimistic records after a definitive backend refusal", async () => {
    const api = nativeApi({ refuse: "A Pi entry ID already exists." });
    const storage = await openDurableStorage({ projectId: "p", conversationId: "c", fetch: api.fetcher });
    await (storage as any).claimWriter(0);
    const result = await storage.commit([initialConversation], () => {}, BACKGROUND_CONTEXT);
    expect(result.stored).toBe(0);
    expect((result.refused as Error).message).toContain("A Pi entry ID already exists.");
    expect(await storage.conversation(ROOT_CONVERSATION_ID, BACKGROUND_CONTEXT)).toBeUndefined();
    expect(api.requests.filter((request) => request.url.endsWith("/commit"))).toHaveLength(1);
  });

  test("refuses writer activation when the history advances after claim", async () => {
    const api = nativeApi({ changeAfterClaim: true });
    const storage = await openDurableStorage({ projectId: "p", conversationId: "c", fetch: api.fetcher });
    await expect((storage as any).claimWriter(0)).rejects.toThrow("Conversation history changed while claiming");
  });

  test("round-trips compaction summary, head provenance and reason through the authenticated batch log", async () => {
    const api = nativeApi();
    const storage = await openDurableStorage({ projectId: "p", conversationId: "c", fetch: api.fetcher });
    await (storage as any).claimWriter(0);
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
});

registerStorageConformance({ describe, expect, it: test }, "authenticated native batch bridge", async (use) => {
  const api = nativeApi();
  const storage = await openDurableStorage({ projectId: "conformance-project", conversationId: "conformance-conversation", fetch: api.fetcher });
  await (storage as any).claimWriter(0);
  try { await use(storage); }
  finally { await storage.close(BACKGROUND_CONTEXT); }
});
