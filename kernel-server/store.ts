import { Database } from "bun:sqlite";
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import type { DeploymentRecord } from "../src/deployments/manifest";
import type { KernelGrant } from "../src/kernel/access";
import type { KernelIdentityStore, KernelSessionRecord } from "../src/kernel/auth";
import type { MiniAppProject } from "../src/lib/types";

export type HostedDeployment = DeploymentRecord & {
  hostname: string;
  files: MiniAppProject["files"];
  accessCodeHash?: string;
};

export type HostedFile = {
  id: string;
  name: string;
  type: string;
  size: number;
  url: string;
  createdAt: number;
};

export class DeploymentStore {
  private readonly deployments = new Map<string, HostedDeployment>();
  private readonly fileWriteQueues = new Map<string, Promise<void>>();

  constructor(private readonly root: string) {}

  async initialize() {
    await mkdir(this.root, { recursive: true });
    for (const entry of await readdir(this.root, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      try {
        const deployment = JSON.parse(await readFile(path.join(this.root, entry.name, "deployment.json"), "utf8")) as HostedDeployment;
        this.deployments.set(deployment.hostname, deployment);
      } catch {
        // Incomplete deployment directories are ignored and can be inspected manually.
      }
    }
  }

  async save(deployment: HostedDeployment) {
    const directory = this.directory(deployment.id);
    await mkdir(directory, { recursive: true });
    await writeFile(path.join(directory, "deployment.json"), JSON.stringify(deployment, null, 2), { mode: 0o600 });
    this.deployments.set(deployment.hostname, deployment);
  }

  findByHostname(hostname: string) {
    return this.deployments.get(hostname) ?? null;
  }

  findById(id: string) {
    return [...this.deployments.values()].find((deployment) => deployment.id === id) ?? null;
  }

  databasePath(deploymentId: string) {
    return path.join(this.directory(deploymentId), "database.sqlite");
  }

  async listFiles(deploymentId: string): Promise<HostedFile[]> {
    try {
      return JSON.parse(await readFile(path.join(this.filesDirectory(deploymentId), "index.json"), "utf8")) as HostedFile[];
    } catch {
      return [];
    }
  }

  async saveFile(deploymentId: string, file: HostedFile, bytes: Uint8Array, totalLimit: number) {
    const previous = this.fileWriteQueues.get(deploymentId) ?? Promise.resolve();
    const write = previous.catch(() => undefined).then(async () => {
      const directory = this.filesDirectory(deploymentId);
      await mkdir(directory, { recursive: true });
      const current = await this.listFiles(deploymentId);
      if (current.reduce((total, item) => total + item.size, 0) + file.size > totalLimit) {
        throw new Error("This deployment has reached its file quota.");
      }
      await writeFile(path.join(directory, `${file.id}.blob`), bytes, { mode: 0o600 });
      await writeFile(path.join(directory, "index.json"), JSON.stringify([file, ...current], null, 2), { mode: 0o600 });
    });
    this.fileWriteQueues.set(deploymentId, write);
    try {
      await write;
    } finally {
      if (this.fileWriteQueues.get(deploymentId) === write) this.fileWriteQueues.delete(deploymentId);
    }
  }

  async readFile(deploymentId: string, fileId: string) {
    const metadata = (await this.listFiles(deploymentId)).find((file) => file.id === fileId);
    if (!metadata) return null;
    const bytes = await readFile(path.join(this.filesDirectory(deploymentId), `${metadata.id}.blob`));
    return { metadata, bytes };
  }

  private directory(deploymentId: string) {
    return path.join(this.root, deploymentId);
  }

  private filesDirectory(deploymentId: string) {
    return path.join(this.directory(deploymentId), "files");
  }
}

export class SqliteKernelIdentityStore implements KernelIdentityStore {
  private readonly db: Database;

  constructor(filename: string) {
    this.db = new Database(filename, { create: true, strict: true });
    this.db.exec(`
      PRAGMA journal_mode = WAL;
      CREATE TABLE IF NOT EXISTS kernel_sessions (
        id TEXT PRIMARY KEY,
        actor_id TEXT NOT NULL,
        expires_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS kernel_grants (
        actor_id TEXT NOT NULL,
        type TEXT NOT NULL CHECK(type IN ('deployment', 'project-owner')),
        resource_id TEXT NOT NULL,
        PRIMARY KEY (actor_id, type, resource_id)
      );
    `);
  }

  async read(id: string): Promise<KernelSessionRecord | null> {
    const row = this.db.query<{ id: string; actor_id: string; expires_at: number }, [string]>(
      "SELECT id, actor_id, expires_at FROM kernel_sessions WHERE id = ?",
    ).get(id);
    return row ? { id: row.id, actorId: row.actor_id, expiresAt: row.expires_at } : null;
  }

  async write(session: KernelSessionRecord) {
    this.db.query("INSERT OR REPLACE INTO kernel_sessions (id, actor_id, expires_at) VALUES (?, ?, ?)")
      .run(session.id, session.actorId, session.expiresAt);
  }

  async delete(id: string) {
    this.db.query("DELETE FROM kernel_sessions WHERE id = ?").run(id);
  }

  async readGrants(actorId: string): Promise<readonly KernelGrant[]> {
    return this.db.query<{ type: "deployment" | "project-owner"; resource_id: string }, [string]>(
      "SELECT type, resource_id FROM kernel_grants WHERE actor_id = ?",
    ).all(actorId).map((row) => row.type === "deployment"
      ? { type: "deployment", deploymentId: row.resource_id }
      : { type: "project-owner", projectId: row.resource_id });
  }

  grant(actorId: string, grant: KernelGrant) {
    const resourceId = grant.type === "deployment" ? grant.deploymentId : grant.projectId;
    this.db.query("INSERT OR IGNORE INTO kernel_grants (actor_id, type, resource_id) VALUES (?, ?, ?)")
      .run(actorId, grant.type, resourceId);
  }
}
