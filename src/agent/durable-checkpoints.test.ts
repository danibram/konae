import { describe, expect, test } from "bun:test";
import { createCheckpointDraft, proposalDisposition, reconcileInterruptedApply, recoverCheckpoint } from "./durable-checkpoints";

async function sha(bytes: Uint8Array) {
  const value = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return [...value].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

describe("Durable recovery checkpoints", () => {
  test("retains exact model context, validation inputs, and binary source bytes across recovery", async () => {
    const binary = new Uint8Array([0, 255, 17, 128]);
    const b64 = btoa(String.fromCharCode(...binary));
    const draft = await createCheckpointDraft({
      projectId: "p", sourcesRevision: 7,
      base: { files: { "index.html": "<p>base</p>" } },
      scratch: { files: { "index.html": "<p>candidate</p>", "public/mark.bin": b64 }, encodings: { "public/mark.bin": "base64" } },
      modelView: { files: { "index.html": "<p>candidate</p>" } },
      modelContext: [{ role: "user", content: "keep the exact boundary" }, { role: "assistant", content: "I will edit the page." }],
      nativeSequence: 19,
      validationSettings: { actions: true, fileAccess: "app-users" }, historicalGate: { ok: true, checked: ["sources"] }, status: "proposal", candidateId: "candidate-1",
    });
    const stored = new Map<string, Uint8Array>();
    for (const item of draft.blobs ?? []) stored.set(item.key, item.bytes);
    const blobFiles = Object.fromEntries(await Promise.all([...stored].map(async ([key, bytes]) => [key, await sha(bytes)] as const)));
    const payload = { ...draft.payload, blobFiles };
    const byDigest = new Map<string, Uint8Array>();
    for (const [key, digest] of Object.entries(blobFiles)) byDigest.set(digest, stored.get(key)!);
    const modelContext = { messages: [{ role: "user", content: "keep the exact boundary" }, { role: "assistant", content: "I will edit the page." }] };
    const recovered = await recoverCheckpoint({ checkpointId: "cp-7", sequence: 19, createdAt: 1, payload }, async (digest) => byDigest.get(digest)!, { revision: 7, files: { files: {} } }, async (sequence) => sequence === 19 ? { ...modelContext, sequence } : null);
    expect(recovered.readable).toBe(true);
    expect(recovered.modelContext).toEqual({ ...modelContext, sequence: 19 });
    expect(recovered.scratch.files["public/mark.bin"]).toBe(b64);
    expect(recovered.scratch.encodings).toEqual({ "public/mark.bin": "base64" });
    expect(recovered.applyAllowed).toBe(false);
    expect(recovered.historicalGate).toEqual({ ok: true, checked: ["sources"] });
  });

  test("preserves stale candidate history and refuses to make historical Gate permission current", async () => {
    const bytes = new TextEncoder().encode("candidate");
    const digest = await sha(bytes);
    const modelContext = { messages: [{ role: "user", content: "prior" }] };
    const payload = { formatVersion: 1, sourcesRevision: 3, status: "proposal", validationSettings: {}, historicalGate: { ok: true }, modelContext: { sequence: 4, kind: "native-prefix" }, blobs: [digest], blobFiles: { "base:index.html": digest, "scratch:index.html": digest, "modelView:index.html": digest },
      base: { manifest: { "index.html": { blobKey: "base:index.html", encoding: "utf8", digest, byteLength: bytes.length } } },
      scratch: { manifest: { "index.html": { blobKey: "scratch:index.html", encoding: "utf8", digest, byteLength: bytes.length } } },
      modelView: { manifest: { "index.html": { blobKey: "modelView:index.html", encoding: "utf8", digest, byteLength: bytes.length } } },
      };
    const recovered = await recoverCheckpoint({ checkpointId: "old", sequence: 4, createdAt: 0, payload }, async () => bytes, { revision: 4, files: { files: { "index.html": "hand-edited" } } }, async () => ({ ...modelContext, sequence: 4 }));
    expect(recovered.stale).toBe(true);
    expect(recovered.status).toBe("proposal");
    expect(recovered.scratch.files["index.html"]).toBe("candidate");
    expect(recovered.applyAllowed).toBe(false);
  });

  test("keeps unknown format and unreadable assets inspectable without execution", async () => {
    const unknown = await recoverCheckpoint({ checkpointId: "future", sequence: 1, createdAt: 0, payload: { formatVersion: 99, sourcesRevision: 1, status: "proposal" } }, async () => new Uint8Array(), { revision: 1, files: { files: {} } }, async () => []);
    expect(unknown.readable).toBe(false);
    expect(unknown.error).toContain("unsupported");
    const broken = await recoverCheckpoint({ checkpointId: "broken", sequence: 2, createdAt: 0, payload: { formatVersion: 1, sourcesRevision: 1, status: "proposal", validationSettings: {}, blobs: [], blobFiles: {}, base: { manifest: { "index.html": { blobKey: "missing", encoding: "utf8", digest: "bad", byteLength: 4 } } }, scratch: { manifest: {} }, modelView: { manifest: {} }, modelContext: { sequence: 2, kind: "native-prefix" } } }, async () => { throw new Error("missing asset"); }, { revision: 1, files: { files: {} } }, async () => ({ sequence: 2, messages: [] }));
    expect(broken.readable).toBe(false);
    expect(broken.error).toContain("file reference");
  });

  test("keeps the immutable file diff inspectable when the native context boundary is unreadable", async () => {
    const bytes = new TextEncoder().encode("preserved candidate");
    const digest = await sha(bytes);
    const payload = { formatVersion: 1, sourcesRevision: 1, status: "proposal", validationSettings: {}, blobs: [digest], blobFiles: { "base:index.html": digest, "scratch:index.html": digest, "modelView:index.html": digest }, modelContext: { sequence: 5, kind: "native-prefix" },
      base: { manifest: { "index.html": { blobKey: "base:index.html", encoding: "utf8", digest, byteLength: bytes.length } } },
      scratch: { manifest: { "index.html": { blobKey: "scratch:index.html", encoding: "utf8", digest, byteLength: bytes.length } } },
      modelView: { manifest: { "index.html": { blobKey: "modelView:index.html", encoding: "utf8", digest, byteLength: bytes.length } } },
    };
    const recovered = await recoverCheckpoint({ checkpointId: "cp-5", sequence: 5, createdAt: 1, payload }, async () => bytes, { revision: 2, files: { files: {} } }, async () => { throw new Error("native prefix advanced"); });
    expect(recovered.readable).toBe(false);
    expect(recovered.stale).toBe(true);
    expect(recovered.scratch.files["index.html"]).toBe("preserved candidate");
  });

  test("reconciles accepted source read-back, unresolved acknowledgements, and conflicting edits", async () => {
    const checkpoint = { sourcesRevision: 8, base: { files: { "index.html": "before" } }, scratch: { files: { "index.html": "candidate" } } };
    expect(await reconcileInterruptedApply(checkpoint, { revision: 9, files: checkpoint.scratch })).toBe("saved");
    expect(await reconcileInterruptedApply(checkpoint, { revision: 8, files: checkpoint.base })).toBe("unchanged");
    expect(await reconcileInterruptedApply(checkpoint, { revision: 10, files: { files: { "index.html": "imported" } } })).toBe("conflict");
    expect(await reconcileInterruptedApply(checkpoint, null)).toBe("unknown");
  });

  test("makes Discard part of the next model context", () => {
    const message = proposalDisposition("discarded", { files: { "index.html": "rejected candidate" } });
    expect(message).toContain("proposal was discarded");
    expect(message).toContain("were rejected");
    expect(message).toContain("current saved files");
  });

  test("names an unobserved Apply outcome without claiming it was saved", () => {
    const message = proposalDisposition("apply-unknown", { files: { "index.html": "candidate" } });
    expect(message).toContain("outcome is unobserved");
    expect(message).toContain("do not repeat the write");
    expect(message).not.toContain("was saved and read back");
  });
});
