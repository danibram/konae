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

/**
 * Publishes the project. `initialSignupPolicy` goes only with a first release:
 * the Kernel installs it before the app serves, and refuses it on a republish.
 */
export async function deployProject(project: MiniAppProject, visibility: DeploymentVisibility, artifact: BuildArtifact, initialSignupPolicy?: Pick<SignupPolicy, "mode" | "allowlist">): Promise<DeploymentReceipt> {
  await platformMcp.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest, sourceAuthority: "managed" });
  const transfer = await platformMcp.upload(new TextEncoder().encode(JSON.stringify({ project, visibility, artifact, ...(initialSignupPolicy ? { initialSignupPolicy } : {}) })), project.id);
  try {
    const plan = await platformMcp.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    return await platformMcp.call<DeploymentReceipt>("release_apply", { planId: plan.planId });
  } finally {
    await platformMcp.cancel(transfer.transferId).catch(() => {});
  }
}

/**
 * Who may create an account on a deployed app, in the order the editor offers
 * them. `invitation` admits only people the owner invited; `allowlist` holds
 * exact emails and `@domain` entries. The Kernel's `signup_policy_set` accepts
 * exactly these modes, and tests/mcp.integration.test.ts checks that.
 */
export const signupModes = ["open", "invitation", "closed", "allowlist"] as const;
export type SignupMode = (typeof signupModes)[number];
export type SignupPolicy = { deploymentId: string; visibility: DeploymentVisibility; mode: SignupMode; allowlist: string[] };

export function readSignupPolicy(deploymentId: string) {
  return platformMcp.call<SignupPolicy>("signup_policy_read", { deploymentId });
}

/** Saves the policy. The Kernel trims, lowercases and deduplicates the entries. */
export function saveSignupPolicy(deploymentId: string, mode: SignupMode, allowlist: string[]) {
  return platformMcp.call<SignupPolicy>("signup_policy_set", { deploymentId, mode, allowlist });
}

// App accounts and invitations. Every tool needs the manage_users permission,
// which the owner's browser session holds. docs/mcp.md, "Users and invitations".

export type AppUserState = "pending" | "active" | "suspended";
export type AppUser = { userId: string; email: string; state: AppUserState; verified: boolean; verifiedAt: number | null; createdAt: number };
export type AppUserPage = { users: AppUser[]; nextCursor: string | null };

export function listAppUsers(deploymentId: string, options: { limit?: number; cursor?: string; state?: AppUserState } = {}) {
  return platformMcp.call<AppUserPage>("app_users_list", { deploymentId, ...options });
}

export function suspendAppUser(deploymentId: string, userId: string) {
  return platformMcp.call<{ userId: string; email: string; state: AppUserState }>("app_user_suspend", { deploymentId, userId });
}

export function resumeAppUser(deploymentId: string, userId: string) {
  return platformMcp.call<{ userId: string; email: string; state: AppUserState }>("app_user_resume", { deploymentId, userId });
}

export type InvitationStatus = "pending" | "expired" | "needs-resend" | "already-member" | "accepted" | "revoked";
/** Whether the mail transport took the message. The Kernel keeps no per-invitation provider state. */
export type InvitationDelivery = "queued" | "failed" | "not-sent";
export type AppInvitation = {
  invitationId: string; email: string; status: InvitationStatus; invitedBy: string;
  createdAt: number; expiresAt: number; acceptedAt: number | null; revokedAt: number | null;
  delivery: InvitationDelivery; sends: number;
};
export type InvitationResult =
  | { email: string; status: "invited"; invitationId: string; expiresAt: number; acceptable: boolean; delivery: InvitationDelivery }
  | { email: string; status: "already-invited"; invitationId: string; acceptable: boolean }
  | { email: string; status: "already-member" | "suspended" | "invalid-email" };

export function createAppInvitations(deploymentId: string, emails: string[], idempotencyKey: string) {
  return platformMcp.call<{ deploymentId: string; policy: SignupMode; results: InvitationResult[] }>("app_invitations_create", { deploymentId, emails, idempotencyKey });
}

export function listAppInvitations(deploymentId: string, options: { limit?: number; cursor?: string } = {}) {
  return platformMcp.call<{ invitations: AppInvitation[]; nextCursor: string | null }>("app_invitations_list", { deploymentId, ...options });
}

export function resendAppInvitation(deploymentId: string, invitationId: string, idempotencyKey: string) {
  return platformMcp.call<{ invitationId: string; email: string; status: "invited"; expiresAt: number; delivery: InvitationDelivery }>("app_invitation_resend", { deploymentId, invitationId, idempotencyKey });
}

export function revokeAppInvitation(deploymentId: string, invitationId: string) {
  return platformMcp.call<{ invitationId: string; email: string; status: "revoked" }>("app_invitation_revoke", { deploymentId, invitationId });
}

/** The installation's mail transport, as `mail_status` reports it. It names no recipient. */
export type MailStatus = {
  transport: string | null;
  sending: boolean;
  reason?: string;
  from?: string;
  preflight?: { state: "checking" | "ready" | "refused"; reason?: string | null; checkedAt?: number | null };
  queue?: Record<string, unknown>;
};

export function readMailStatus(refresh = false) {
  return platformMcp.call<MailStatus>("mail_status", refresh ? { refresh: true } : {});
}

// Signing secrets belong to the project, so they can be set before the first
// release. API keys belong to one Deployment. docs/mcp.md, "HTTP routes".

export type ProjectSecret = { name: string; updatedAt: number };

export function listSecrets(projectId: string) {
  return platformMcp.call<{ projectId: string; secrets: ProjectSecret[] }>("secrets_list", { projectId });
}

export function setSecret(projectId: string, name: string, value: string) {
  return platformMcp.call<{ projectId: string; name: string; updatedAt: number }>("secret_set", { projectId, name, value });
}

export function deleteSecret(projectId: string, name: string) {
  return platformMcp.call<{ projectId: string; name: string; deleted: boolean }>("secret_delete", { projectId, name });
}

export type FileVisibility = "public" | "private";
/** `revokedReason` is "import" for a key that arrived with an imported app, revoked; null when the owner revoked it or it is live. */
export type ApiKey = { keyId: string; name: string; actions: string[]; files: FileVisibility[]; createdAt: number; revokedAt: number | null; revokedReason?: "import" | null };

export function listApiKeys(deploymentId: string) {
  return platformMcp.call<{ deploymentId: string; keys: ApiKey[] }>("api_keys_list", { deploymentId });
}

/** The answer carries the key once. Nothing can read it again. */
export function createApiKey(deploymentId: string, name: string, actions: string[], files: FileVisibility[]) {
  return platformMcp.call<ApiKey & { key: string }>("api_keys_create", { deploymentId, name, actions, files });
}

export function revokeApiKey(deploymentId: string, keyId: string) {
  return platformMcp.call<{ keyId: string; revoked: boolean }>("api_keys_revoke", { deploymentId, keyId });
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
