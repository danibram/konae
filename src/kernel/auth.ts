import type { KernelActor, KernelGrant } from "./access";

export type KernelSessionRecord = {
  id: string;
  actorId: string;
  expiresAt: number;
};

export type KernelIdentityStore = {
  read(id: string): Promise<KernelSessionRecord | null>;
  write(session: KernelSessionRecord): Promise<void>;
  delete(id: string): Promise<void>;
  readGrants(actorId: string): Promise<readonly KernelGrant[]>;
};

export class KernelIdentity {
  constructor(
    private readonly store: KernelIdentityStore,
    private readonly options: { now?: () => number; createToken?: () => string } = {},
  ) {}

  async issueSession(input: { actorId: string; ttlMs?: number }) {
    if (!input.actorId.trim()) throw new Error("A Kernel session requires an actor ID.");
    const token = (this.options.createToken ?? createOpaqueToken)();
    const id = await hashToken(token);
    const expiresAt = this.now() + Math.min(Math.max(input.ttlMs ?? 86_400_000, 60_000), 30 * 86_400_000);
    await this.store.write({ id, actorId: input.actorId, expiresAt });
    return { token, expiresAt };
  }

  async authenticate(token: string | null | undefined): Promise<KernelActor | null> {
    if (!token) return null;
    const id = await hashToken(token);
    const session = await this.store.read(id);
    if (!session) return null;
    if (session.expiresAt <= this.now()) {
      await this.store.delete(id);
      return null;
    }
    return { kind: "authenticated", actorId: session.actorId, grants: await this.store.readGrants(session.actorId) };
  }

  async revokeSession(token: string): Promise<void> {
    await this.store.delete(await hashToken(token));
  }

  private now() {
    return (this.options.now ?? Date.now)();
  }
}

export class MemoryKernelIdentityStore implements KernelIdentityStore {
  private readonly sessions = new Map<string, KernelSessionRecord>();
  private readonly grants = new Map<string, readonly KernelGrant[]>();

  async read(id: string) {
    return this.sessions.get(id) ?? null;
  }

  async write(session: KernelSessionRecord) {
    this.sessions.set(session.id, session);
  }

  async delete(id: string) {
    this.sessions.delete(id);
  }

  async readGrants(actorId: string) {
    return this.grants.get(actorId) ?? [];
  }

  setGrants(actorId: string, grants: readonly KernelGrant[]) {
    this.grants.set(actorId, grants);
  }
}

function createOpaqueToken(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return bytesToBase64Url(bytes);
}

async function hashToken(token: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token));
  return bytesToBase64Url(new Uint8Array(digest));
}

function bytesToBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}
