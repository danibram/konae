import type { DrizzleDatabaseModel, MiniAppProject } from "../lib/types";
import type { DatabaseAdminHost, DatabaseBackup, DatabasePage, DatabaseRunResult, DatabaseTable } from "../runtime/host";
import type { ApplyDatabaseMigrationInput } from "../database-lifecycle/contracts";
import type { SqlValue } from "sql.js";
import type { DeploymentRecord, DeploymentVisibility } from "./manifest";
import type { BuildArtifact } from "../runtime/artifact";

export type DeploymentReceipt = DeploymentRecord & { accessCode?: string; databaseModel?: DrizzleDatabaseModel };

export type PortableProjectState = {
  initialDatabase: Uint8Array;
  initialIdentity?: Uint8Array;
  initialFiles?: Uint8Array;
};

const adminKey = import.meta.env.VITE_LITTLEBOX_ADMIN_KEY ?? "local-development-key";

export async function deployProject(project: MiniAppProject, visibility: DeploymentVisibility, artifact: BuildArtifact): Promise<DeploymentReceipt> {
  const response = await fetch("/_littlebox/admin/deploy", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-littlebox-admin-key": adminKey,
    },
    body: JSON.stringify({ project, visibility, artifact }),
  });
  const body = await response.json().catch(() => ({})) as { result?: DeploymentReceipt; error?: string };
  if (!response.ok || !body.result) throw new Error(body.error ?? "The Littlebox Kernel could not publish this deployment.");
  return body.result;
}

export class HostedDatabaseClient implements DatabaseAdminHost {
  constructor(private readonly deploymentId: string) {}

  inspectDatabase() {
    return this.request<DatabaseTable[]>({ operation: "inspect" });
  }

  runSql(sql: string) {
    return this.request<DatabaseRunResult>({ operation: "sql", sql });
  }

  browseTable(tableName: string, options: { page?: number; pageSize?: number; sortColumn?: string; sortDirection?: "ASC" | "DESC" } = {}) {
    return this.request<DatabasePage>({ operation: "browse", tableName, ...options });
  }

  updateCell(tableName: string, rowId: SqlValue, columnName: string, value: SqlValue) {
    return this.request({ operation: "update-cell", tableName, rowId, columnName, value });
  }

  insertRow(tableName: string, values: Record<string, SqlValue>) {
    return this.request({ operation: "insert-row", tableName, values });
  }

  deleteRow(tableName: string, rowId: SqlValue) {
    return this.request({ operation: "delete-row", tableName, rowId });
  }

  listDatabaseBackups() {
    return this.request<DatabaseBackup[]>({ operation: "list-backups" });
  }

  createDatabaseBackup(label = "Manual backup", model?: { source: string; revision: string }) {
    return this.request<DatabaseBackup>({ operation: "create-backup", label, model });
  }

  restoreDatabaseBackup(backupId: string) {
    return this.request<DatabaseBackup>({ operation: "restore-backup", backupId });
  }

  applyDatabaseMigration(input: ApplyDatabaseMigrationInput) {
    return this.request<{ backup: DatabaseBackup }>({ operation: "apply-migration", ...input });
  }

  async exportDatabase() {
    const response = await fetch(`${this.endpoint}/export`, { headers: { "x-littlebox-admin-key": adminKey } });
    if (!response.ok) throw new Error(await response.text() || "Could not export the deployed database.");
    return new Uint8Array(await response.arrayBuffer());
  }

  async exportPortableState(): Promise<PortableProjectState> {
    const response = await fetch(`${this.deploymentEndpoint}/portable-state`, { headers: { "x-littlebox-admin-key": adminKey } });
    const raw = await response.text();
    const body = (() => {
      try {
        return JSON.parse(raw) as { result?: { initialDatabase?: string; initialIdentity?: string; initialFiles?: string }; error?: string };
      } catch {
        return {};
      }
    })();
    if (!response.ok || !body.result?.initialDatabase) {
      throw new Error(body.error ?? (raw.trim() || "Could not export portable app state."));
    }
    return {
      initialDatabase: decodeBase64(body.result.initialDatabase),
      ...(body.result.initialIdentity ? { initialIdentity: decodeBase64(body.result.initialIdentity) } : {}),
      ...(body.result.initialFiles ? { initialFiles: decodeBase64(body.result.initialFiles) } : {}),
    };
  }

  private async request<T = unknown>(payload: Record<string, unknown>): Promise<T> {
    const response = await fetch(this.endpoint, {
      method: "POST",
      headers: { "content-type": "application/json", "x-littlebox-admin-key": adminKey },
      body: JSON.stringify(payload),
    });
    const body = await response.json().catch(() => ({})) as { result?: T; error?: string };
    if (!response.ok || body.result === undefined) throw new Error(body.error ?? "Deployed database request failed.");
    return body.result;
  }

  private get endpoint() {
    return `${this.deploymentEndpoint}/database`;
  }

  private get deploymentEndpoint() {
    return `/_littlebox/admin/deployments/${encodeURIComponent(this.deploymentId)}`;
  }
}

function decodeBase64(value: string): Uint8Array {
  const normalized = value.replaceAll("-", "+").replaceAll("_", "/");
  const binary = atob(normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "="));
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}
