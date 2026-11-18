import { describe, expect, test } from "bun:test";
import { sourceSavePayload } from "./project-sources";
import type { MiniAppProject } from "./types";

const project = (files: Record<string, string>): MiniAppProject => ({ id: "p", name: "P", slug: "p", files, directories: [], manifest: { version: 1, visibility: "private" }, updatedAt: 1 });

describe("pinned recovered source saves", () => {
  test("uses a delta only when its base is the pinned source revision", () => {
    const previous = project({ "index.html": "old", "gone.js": "remove" });
    const candidate = project({ "index.html": "new" });
    const delta = sourceSavePayload(previous, candidate, 5, 5, true) as any;
    expect(delta.format).toBe("delta");
    expect(delta.files).toEqual({ "index.html": "new" });
    expect(delta.removedPaths).toEqual(["gone.js"]);
    const fullAfterNewerTab = sourceSavePayload(previous, candidate, 6, 5, true);
    expect(fullAfterNewerTab).toEqual(candidate);
    const fullWhenPinnedBaseWasNotLoaded = sourceSavePayload(previous, candidate, 6, 5, true);
    expect(fullWhenPinnedBaseWasNotLoaded).toEqual(candidate);
  });

  test("a queued same-tab hand edit is kept ahead of a recovered proposal", async () => {
    let releaseTransfer!: () => void;
    let transferStarted!: () => void;
    const started = new Promise<void>((resolve) => transferStarted = resolve);
    const gate = new Promise<void>((resolve) => releaseTransfer = resolve);
    let saveRevision = 5;
    let transferCount = 0;
    const fetcher: typeof fetch = async (_input, init = {}) => {
      const request = JSON.parse(String(init.body)) as { id: string; method: string; params?: { name?: string } };
      if (request.method === "initialize") return Response.json({ jsonrpc: "2.0", id: request.id, result: { protocolVersion: "2025-11-25" } });
      if (request.method === "notifications/initialized") return new Response(null, { status: 202 });
      const name = request.params?.name;
      let result: unknown = {};
      if (name === "transfer_begin") {
        transferCount += 1;
        if (transferCount === 1) { transferStarted(); await gate; }
        result = { transferId: `transfer-${transferCount}`, size: 1, sha256: "0".repeat(64), chunkBytes: 4 };
      } else if (name === "transfer_write" || name === "transfer_complete" || name === "transfer_cancel") result = {};
      else if (name === "project_sources_save") result = { revision: ++saveRevision };
      return Response.json({ jsonrpc: "2.0", id: request.id, result: { structuredContent: { result } } });
    };
    const sources = new (await import("./project-sources")).ProjectSources("owner", "/mcp", fetcher);
    const base = project({ "index.html": "base" });
    sources.markPublished("p", 5, base);
    const handEditA = project({ "index.html": "hand edit A" });
    const handEditB = project({ "index.html": "hand edit B" });
    const recovered = project({ "index.html": "recovered candidate" });
    const first = sources.save(handEditA);
    await started;
    const queuedHandEdit = sources.save(handEditB);
    await expect(sources.saveExpected(recovered, 5)).rejects.toThrow("A hand-edited source save is already queued");
    releaseTransfer();
    await expect(first).resolves.toBe(6);
    await expect(queuedHandEdit).resolves.toBe(7);
    expect(transferCount).toBe(2);
    expect(saveRevision).toBe(7);
  });
});
