import { describe, expect, test } from "bun:test";
import type { KernelGrant } from "./access";
import { KernelIdentity, type KernelIdentityStore, type KernelSessionRecord } from "./auth";

class RecordingStore implements KernelIdentityStore {
  records = new Map<string, KernelSessionRecord>();
  grants = new Map<string, readonly KernelGrant[]>();
  async read(id: string) { return this.records.get(id) ?? null; }
  async write(session: KernelSessionRecord) { this.records.set(session.id, session); }
  async delete(id: string) { this.records.delete(id); }
  async readGrants(actorId: string) { return this.grants.get(actorId) ?? []; }
}

describe("Konae Kernel identity", () => {
  test("issues opaque sessions and stores only a token hash", async () => {
    const store = new RecordingStore();
    store.grants.set("user_1", [{ type: "deployment", deploymentId: "deploy_1" }]);
    const identity = new KernelIdentity(store, { now: () => 1_000, createToken: () => "raw-secret-token" });
    const session = await identity.issueSession({
      actorId: "user_1",
      ttlMs: 60_000,
    });

    expect(session.token).toBe("raw-secret-token");
    expect([...store.records.keys()]).not.toContain("raw-secret-token");
    expect(await identity.authenticate(session.token)).toEqual({
      kind: "authenticated",
      actorId: "user_1",
      grants: [{ type: "deployment", deploymentId: "deploy_1" }],
    });
  });

  test("expires and revokes sessions inside the Kernel", async () => {
    let now = 1_000;
    const store = new RecordingStore();
    const identity = new KernelIdentity(store, { now: () => now, createToken: () => "session-token" });
    const session = await identity.issueSession({ actorId: "user_1", ttlMs: 60_000 });
    now = session.expiresAt;
    expect(await identity.authenticate(session.token)).toBeNull();

    now = 1_000;
    const active = await identity.issueSession({ actorId: "user_1", ttlMs: 60_000 });
    await identity.revokeSession(active.token);
    expect(await identity.authenticate(active.token)).toBeNull();
  });

  test("reads current grants instead of freezing them in the session", async () => {
    const store = new RecordingStore();
    const identity = new KernelIdentity(store, { now: () => 1_000, createToken: () => "session-token" });
    const session = await identity.issueSession({ actorId: "user_1" });
    expect((await identity.authenticate(session.token))?.grants).toEqual([]);

    store.grants.set("user_1", [{ type: "deployment", deploymentId: "deploy_1" }]);
    expect((await identity.authenticate(session.token))?.grants).toEqual([{ type: "deployment", deploymentId: "deploy_1" }]);
    store.grants.set("user_1", []);
    expect((await identity.authenticate(session.token))?.grants).toEqual([]);
  });

  test("rejects empty identities and clamps session lifetime", async () => {
    const identity = new KernelIdentity(new RecordingStore(), { now: () => 0, createToken: () => "token" });
    await expect(identity.issueSession({ actorId: " " })).rejects.toThrow();
    expect((await identity.issueSession({ actorId: "user", ttlMs: 1 })).expiresAt).toBe(60_000);
  });
});
