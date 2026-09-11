import type { DrizzleDatabaseModel, MiniAppProject } from "../lib/types";
import type { DatabaseAdminHost, DatabaseBackup, DatabasePage, DatabaseRunResult, DatabaseTable } from "../runtime/host";
import type { ApplyDatabaseMigrationInput } from "../database-lifecycle/contracts";
import type { SqlValue } from "sql.js";
import type { DeploymentRecord, DeploymentVisibility } from "./manifest";
import type { BuildArtifact } from "../runtime/artifact";
import { platformMcp, decodeBase64, type Transfer } from "../mcp/client";

export type DeploymentReceipt = DeploymentRecord & { accessCode?: string; databaseModel?: DrizzleDatabaseModel };

export type PortableProjectState = {
  visibility?: DeploymentVisibility;
  initialDatabase: Uint8Array;
  initialIdentity?: Uint8Array;
  initialFiles?: Uint8Array;
  initialSignupPolicy?: Pick<SignupPolicy, "mode" | "allowlist">;
};

export async function deployProject(project: MiniAppProject, visibility: DeploymentVisibility, artifact: BuildArtifact): Promise<DeploymentReceipt> {
  await platformMcp.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest, sourceAuthority: "managed" });
  const transfer = await platformMcp.upload(new TextEncoder().encode(JSON.stringify({ project, visibility, artifact })), project.id);
  try {
    const plan = await platformMcp.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    return await platformMcp.call<DeploymentReceipt>("release_apply", { planId: plan.planId });
  } finally {
    await platformMcp.cancel(transfer.transferId).catch(() => {});
  }
}

/** Who may create an account on a deployed app. `allowlist` holds exact emails and `@domain` entries. */
export type SignupMode = "open" | "allowlist" | "closed";
export type SignupPolicy = { deploymentId: string; visibility: DeploymentVisibility; mode: SignupMode; allowlist: string[] };

export function readSignupPolicy(deploymentId: string) {
  return platformMcp.call<SignupPolicy>("signup_policy_read", { deploymentId });
}

/** Saves the policy. The Kernel trims, lowercases and deduplicates the entries. */
export function saveSignupPolicy(deploymentId: string, mode: SignupMode, allowlist: string[]) {
  return platformMcp.call<SignupPolicy>("signup_policy_set", { deploymentId, mode, allowlist });
}

export class HostedDatabaseClient implements DatabaseAdminHost {
  constructor(readonly deploymentId: string) {}

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
    const transfer = await platformMcp.call<Transfer>("database_export", { deploymentId: this.deploymentId });
    return platformMcp.download(transfer);
  }

  async exportPortableState(): Promise<PortableProjectState> {
    const transfer = await platformMcp.call<Transfer>("project_export", { deploymentId: this.deploymentId });
    const body = JSON.parse(new TextDecoder().decode(await platformMcp.download(transfer))) as { result: { initialDatabase: string; initialIdentity?: string; initialFiles?: string; initialSignupPolicy?: Pick<SignupPolicy, "mode" | "allowlist">; visibility?: DeploymentVisibility } };
    if (!body.result?.initialDatabase) throw new Error("Portable export is missing its database.");
    return {
      ...(body.result.visibility ? { visibility: body.result.visibility } : {}),
      initialDatabase: decodeBase64(body.result.initialDatabase),
      ...(body.result.initialIdentity ? { initialIdentity: decodeBase64(body.result.initialIdentity) } : {}),
      ...(body.result.initialFiles ? { initialFiles: decodeBase64(body.result.initialFiles) } : {}),
      ...(body.result.initialSignupPolicy ? { initialSignupPolicy: body.result.initialSignupPolicy } : {}),
    };
  }

  private async request<T = unknown>(payload: Record<string, unknown>): Promise<T> {
    const { operation, ...input } = payload;
    return platformMcp.call<T>("database_execute", { deploymentId: this.deploymentId, operation, input });
  }
}
