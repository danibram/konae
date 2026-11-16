import type { MiniAppProject } from "../lib/types";
import type { DatabaseAdminHost, DatabaseBackup, DatabasePage, DatabaseRunResult, DatabaseTable } from "../runtime/host";
import type { SqlValue } from "sql.js";
import type { DeploymentRecord, DeploymentVisibility } from "./manifest";
import type { BuildArtifact } from "../runtime/artifact";
import { defaultModuleStates } from "../lib/app-folder";
import { McpError, platformMcp, type Transfer } from "../mcp/client";
import { publishProject, type ConfirmSchema } from "./release";

export type DeploymentReceipt = DeploymentRecord;

export function isSettingChanged(error: unknown): boolean {
  return error instanceof McpError && error.refusal?.code === "setting.changed";
}

export function isModuleChanged(error: unknown): boolean {
  return error instanceof McpError && error.refusal?.code === "module.changed";
}

export type ModuleRead = {
  projectId: string;
  modules: Record<string, {
    state: Record<string, unknown>;
    switch: boolean;
    message: string | null;
    summary: string;
    options: { key: string; type: string; values?: string[]; default: unknown; label: string; help: string }[];
    current?: unknown;
  }>;
};

export function readModules(projectId: string) {
  return platformMcp.call<ModuleRead>("modules_read", { projectId });
}

export async function setModule(projectId: string, name: string, state: Record<string, unknown>, on?: boolean, options?: Record<string, unknown>) {
  await platformMcp.call("module_set", { projectId, module: name, ...(on === undefined ? {} : { on }), ...(options ? { options } : {}), expected: state });
  return readModules(projectId);
}


export async function deployProject(project: MiniAppProject, visibility: DeploymentVisibility, artifact: BuildArtifact, confirmSchema: ConfirmSchema, initialSignupPolicy?: Pick<SignupPolicy, "mode" | "allowlist">, observedSourcesRevision?: number, initialModules?: MiniAppProject["modules"]): Promise<{ receipt: DeploymentReceipt; sourcesRevision: number }> {
  const { plan, receipt } = await publishProject<DeploymentReceipt>(platformMcp, project, () => ({ project, visibility, artifact, initialSignupPolicy, ...(initialModules ? { initialModules: defaultModuleStates(initialModules) } : {}) }), confirmSchema, () => {}, undefined, observedSourcesRevision);
  return { receipt, sourcesRevision: plan.expectedSourcesRevision + 1 };
}




export const signupModes = ["open", "invitation", "closed", "allowlist"] as const;
export type SignupMode = (typeof signupModes)[number];
export type SignupPolicy = { projectId: string; visibility: DeploymentVisibility; mode: SignupMode; allowlist: string[] };

export async function readSignupPolicy(projectId: string) {
  const inspected = await platformMcp.call<{ deployment: { id: string; manifest: { visibility: DeploymentVisibility }; signupPolicy: { mode: SignupMode; allowlist?: string[] } } | null }>("project_inspect", { projectId });
  if (!inspected.deployment) throw new Error("Publish this project before reading its Signup Policy.");
  return { projectId, visibility: inspected.deployment.manifest.visibility, mode: inspected.deployment.signupPolicy.mode, allowlist: inspected.deployment.signupPolicy.allowlist ?? [] };
}


export async function saveSignupPolicy(projectId: string, mode: SignupMode, allowlist: string[]) {
  await platformMcp.call("signup_policy_set", { projectId, mode, allowlist });
  return readSignupPolicy(projectId);
}


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
  return platformMcp.call<SignInMethods>("sign_in_methods_read", { projectId: deploymentId });
}

export function saveSignInMethods(deploymentId: string, google: boolean) {
  const methods: SignInMethod[] = google ? ["password", "google"] : ["password"];
  return platformMcp.call<SignInMethods>("sign_in_methods_set", { projectId: deploymentId, methods });
}


export const GOOGLE_CLIENT_ID = "google_client_id";
export const GOOGLE_CLIENT_SECRET = "google_client_secret";

export function setGoogleClientPart(projectId: string, name: typeof GOOGLE_CLIENT_ID | typeof GOOGLE_CLIENT_SECRET, value: string) {
  return platformMcp.call<{ projectId: string; name: string; updatedAt: number }>("secret_set", { projectId, name, value, use: "google" });
}




export type AppUserState = "pending" | "active" | "suspended";
export type AppUser = { userId: string; email: string; state: AppUserState; verified: boolean; verifiedAt: number | null; createdAt: number };
export type AppUserPage = { users: AppUser[]; nextCursor: string | null };

export function listAppUsers(deploymentId: string, options: { limit?: number; cursor?: string; state?: AppUserState } = {}) {
  return platformMcp.call<AppUserPage>("app_users_list", { projectId: deploymentId, ...options });
}

export function suspendAppUser(deploymentId: string, userId: string) {
  return platformMcp.call<{ userId: string; email: string; state: AppUserState }>("app_user_suspend", { projectId: deploymentId, userId });
}

export function resumeAppUser(deploymentId: string, userId: string) {
  return platformMcp.call<{ userId: string; email: string; state: AppUserState }>("app_user_resume", { projectId: deploymentId, userId });
}


export function enterAsAppUser(deploymentId: string, userId: string) {
  return platformMcp.call<{ userId: string; email: string; url: string; expiresAt: number }>("app_user_enter", { projectId: deploymentId, userId });
}

export type InvitationStatus = "pending" | "expired" | "needs-resend" | "already-member" | "accepted" | "revoked";

export type InvitationDelivery = "queued" | "failed" | "not-sent";
export type AppInvitation = {
  invitationId: string; email: string; status: InvitationStatus; invitedBy: string;
  createdAt: number; expiresAt: number; acceptedAt: number | null; revokedAt: number | null;
  delivery: InvitationDelivery; sends: number;
};

export type InvitationResult =
  | { email: string; status: "invited"; invitationId: string; expiresAt: number; acceptable: boolean; delivery: InvitationDelivery | "local-mailbox"; acceptUrl?: string }
  | { email: string; status: "already-invited"; invitationId: string; acceptable: boolean }
  | { email: string; status: "already-member" | "suspended" | "invalid-email" };

export function createAppInvitations(deploymentId: string, emails: string[], idempotencyKey: string) {
  return platformMcp.call<{ deploymentId: string; policy: SignupMode; results: InvitationResult[] }>("app_invitations_create", { projectId: deploymentId, emails, idempotencyKey });
}

export function listAppInvitations(deploymentId: string, options: { limit?: number; cursor?: string } = {}) {
  return platformMcp.call<{ invitations: AppInvitation[]; nextCursor: string | null }>("app_invitations_list", { projectId: deploymentId, ...options });
}

export function resendAppInvitation(deploymentId: string, invitationId: string, idempotencyKey: string) {
  return platformMcp.call<{ invitationId: string; email: string; status: "invited"; expiresAt: number; delivery: InvitationDelivery }>("app_invitation_resend", { projectId: deploymentId, invitationId, idempotencyKey });
}

export function revokeAppInvitation(deploymentId: string, invitationId: string) {
  return platformMcp.call<{ invitationId: string; email: string; status: "revoked" }>("app_invitation_revoke", { projectId: deploymentId, invitationId });
}


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





export type ProjectSecret = { name: string; updatedAt: number; use: "route" | "call"; svixKey: boolean };


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

export type ApiKey = { keyId: string; name: string; actions: string[]; files: FileVisibility[]; createdAt: number; revokedAt: number | null; revokedReason?: "import" | null };

export function listApiKeys(deploymentId: string) {
  return platformMcp.call<{ deploymentId: string; keys: ApiKey[] }>("api_keys_list", { projectId: deploymentId });
}


export function createApiKey(deploymentId: string, name: string, actions: string[], files: FileVisibility[]) {
  return platformMcp.call<ApiKey & { key: string }>("api_keys_create", { projectId: deploymentId, name, actions, files });
}

export function revokeApiKey(deploymentId: string, keyId: string) {
  return platformMcp.call<{ keyId: string; revoked: boolean }>("api_keys_revoke", { projectId: deploymentId, keyId });
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


  async exportDatabase() {
    const exported = await platformMcp.call<{ downloads: Record<string, Transfer>; folder: Transfer }>("project_export", { projectId: this.deploymentId });
    try {
      return await platformMcp.download(exported.downloads.database);
    } finally {
      for (const transfer of [exported.folder, ...Object.values(exported.downloads)]) await platformMcp.cancel(transfer.transferId).catch(() => {});
    }
  }

  private async request<T = unknown>(payload: Record<string, unknown>): Promise<T> {
    const { operation, ...input } = payload;
    return platformMcp.call<T>("database_execute", { projectId: this.deploymentId, operation, input });
  }
}


export type MaintenanceState = { maintenance: boolean; since: number | null; unreadable: boolean; installation: string | null };

export function readMaintenance(deploymentId: string) {
  return platformMcp.call<MaintenanceState>("maintenance_read", { projectId: deploymentId });
}


export function setMaintenance(deploymentId: string, on: boolean, expected: boolean) {
  return platformMcp.call<MaintenanceState>("maintenance_set", { projectId: deploymentId, on, expected });
}


export type ContactEmailState = { deploymentId: string; contactEmail: string | null; ownerNotices: string[] };

export function readContactEmail(deploymentId: string) {
  return platformMcp.call<ContactEmailState>("contact_email_read", { projectId: deploymentId });
}


export function setContactEmail(deploymentId: string, email: string | null, expected: string | null) {
  return platformMcp.call<ContactEmailState>("contact_email_set", { projectId: deploymentId, email, expected });
}
