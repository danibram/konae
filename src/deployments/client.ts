import type { MiniAppProject } from "../lib/types";
import type { DatabaseAdminHost, DatabaseBackup, DatabasePage, DatabaseRunResult, DatabaseTable } from "../runtime/host";
import type { SqlValue } from "sql.js";
import type { DeploymentRecord, DeploymentVisibility } from "./manifest";
import type { BuildArtifact } from "../runtime/artifact";
import { platformMcp, type Transfer } from "../mcp/client";
import { release, type ConfirmSchema } from "./release";

export type DeploymentReceipt = DeploymentRecord & { accessCode?: string };

/**
 * Publishes the project. `confirmSchema` is asked when the schema change loses
 * data. `initialSignupPolicy` goes only with a first release: the Kernel
 * installs it before the app serves, and refuses it on a republish.
 */
export async function deployProject(project: MiniAppProject, visibility: DeploymentVisibility, artifact: BuildArtifact, confirmSchema: ConfirmSchema, initialSignupPolicy?: Pick<SignupPolicy, "mode" | "allowlist">): Promise<DeploymentReceipt> {
  await platformMcp.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const transfer = await platformMcp.upload(new TextEncoder().encode(JSON.stringify({ project, visibility, artifact, ...(initialSignupPolicy ? { initialSignupPolicy } : {}) })), project.id);
  try {
    return (await release<DeploymentReceipt>(platformMcp, transfer.transferId, confirmSchema)).receipt;
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

/**
 * How people sign in to a deployed app. Password is always on. `offered` is
 * what the sign-in page shows: google only once the owner turned it on and
 * set the app's Google client. `googleClient` says whether each half is set,
 * never its value. `redirectUris` are the callbacks the owner registers in
 * the Google console. `notice` explains an app that asks for Google without
 * its client here, such as after an import.
 */
export type SignInMethod = "password" | "google";
export type SignInMethods = {
  deploymentId: string;
  methods: SignInMethod[];
  offered: SignInMethod[];
  googleClient: { clientIdSet: boolean; clientSecretSet: boolean };
  redirectUris: string[];
  notice?: string;
};

export function readSignInMethods(deploymentId: string) {
  return platformMcp.call<SignInMethods>("sign_in_methods_read", { deploymentId });
}

export function saveSignInMethods(deploymentId: string, google: boolean) {
  const methods: SignInMethod[] = google ? ["password", "google"] : ["password"];
  return platformMcp.call<SignInMethods>("sign_in_methods_set", { deploymentId, methods });
}

/** The project secrets that hold the app's own Google OAuth client. */
export const GOOGLE_CLIENT_ID = "google_client_id";
export const GOOGLE_CLIENT_SECRET = "google_client_secret";

export function setGoogleClientPart(projectId: string, name: typeof GOOGLE_CLIENT_ID | typeof GOOGLE_CLIENT_SECRET, value: string) {
  return platformMcp.call<{ projectId: string; name: string; updatedAt: number }>("secret_set", { projectId, name, value, use: "google" });
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

/** A one-time link that signs a browser in to the app as this account. It works once, for two minutes. */
export function enterAsAppUser(deploymentId: string, userId: string) {
  return platformMcp.call<{ userId: string; email: string; url: string; expiresAt: number }>("app_user_enter", { deploymentId, userId });
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

/**
 * One row of `secrets_list`. `use` is "call" for a secret a route may not sign
 * with. `svixKey` says whether the value decodes as a svix whsec_ key.
 * The value never leaves the Kernel.
 */
export type ProjectSecret = { name: string; updatedAt: number; use: "route" | "call"; svixKey: boolean };

/**
 * The project's signing secrets. The app's Google client (`use` "google")
 * is left out: no route or call reads it, and Auth > Users shows it.
 */
export async function listSecrets(projectId: string) {
  const answer = await platformMcp.call<{ projectId: string; secrets: Array<Omit<ProjectSecret, "use"> & { use: ProjectSecret["use"] | "google" }> }>("secrets_list", { projectId });
  return { ...answer, secrets: answer.secrets.filter((secret): secret is ProjectSecret => secret.use !== "google") };
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

  createDatabaseBackup(label = "Manual backup") {
    return this.request<DatabaseBackup>({ operation: "create-backup", label });
  }

  restoreDatabaseBackup(backupId: string) {
    return this.request<DatabaseBackup>({ operation: "restore-backup", backupId });
  }

  /** The database as the one export carries it: project_export, then only its database download. */
  async exportDatabase() {
    const exported = await platformMcp.call<{ downloads: Record<string, Transfer>; folder: Transfer }>("project_export", { deploymentId: this.deploymentId });
    try {
      return await platformMcp.download(exported.downloads.database);
    } finally {
      for (const transfer of [exported.folder, ...Object.values(exported.downloads)]) await platformMcp.cancel(transfer.transferId).catch(() => {});
    }
  }

  private async request<T = unknown>(payload: Record<string, unknown>): Promise<T> {
    const { operation, ...input } = payload;
    return platformMcp.call<T>("database_execute", { deploymentId: this.deploymentId, operation, input });
  }
}

/**
 * Whether the app is in maintenance (kernel-rs/kernel/src/maintenance.rs).
 * `since` is when it went in. `installation` is a hold of the whole
 * installation, which taking the app out does not lift.
 */
export type MaintenanceState = { maintenance: boolean; since: number | null; unreadable: boolean; installation: string | null };

export function readMaintenance(deploymentId: string) {
  return platformMcp.call<MaintenanceState>("maintenance_read", { deploymentId });
}

/** Puts the app in maintenance or takes it out, and answers with the state the Kernel kept. */
export function setMaintenance(deploymentId: string, on: boolean) {
  return platformMcp.call<MaintenanceState>("maintenance_set", { deploymentId, on });
}

/**
 * Where `"notify": { "to": "owner" }` sends (kernel-rs/kernel/src/notify.rs).
 * `ownerNotices` names the actions that send there; with no contact email
 * those notices are not sent.
 */
export type ContactEmailState = { deploymentId: string; contactEmail: string | null; ownerNotices: string[] };

export function readContactEmail(deploymentId: string) {
  return platformMcp.call<ContactEmailState>("contact_email_read", { deploymentId });
}

/** Sets the app's contact email in konae.json, or clears it with null, and answers with what the Kernel kept. */
export function setContactEmail(deploymentId: string, email: string | null) {
  return platformMcp.call<ContactEmailState>("contact_email_set", { deploymentId, email });
}
