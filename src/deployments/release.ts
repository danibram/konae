import type { McpClient } from "../mcp/client";

/** What a schema change loses, as the Kernel words it. */
export type SchemaLoss = { table: string; column: string | null; sentence: string };

/**
 * The change `release_plan` derives from the app's live database to the one
 * schema.sql states (kernel-rs/kernel/src/schema_change.rs). `confirmation` is set
 * only when the change loses data, and release_apply then needs it back.
 */
export type SchemaChange = { statements: string[]; losses: SchemaLoss[]; confirmation: string | null };

/** `schema` is absent when release_plan replays an import it already applied. */
export type ReleasePlan = { planId: string; projectId: string; expectedRevision: number; schema?: SchemaChange };

/** Asks the owner about a schema change that loses data. False stops the release. */
export type ConfirmSchema = (change: SchemaChange) => Promise<boolean>;

/** The release stopped because the owner kept the data a schema change would lose. */
export class SchemaChangeDeclined extends Error {
  constructor(readonly change: SchemaChange) {
    super(`Nothing was published. ${change.losses.map((loss) => loss.sentence).join(" ")} Keep the table or column in schema.sql, or confirm the change.`);
    this.name = "SchemaChangeDeclined";
  }
}

/**
 * Releases with release_plan, then release_apply. The editor, `konae publish` and
 * `konae dev` all release through here, so each one shows the same plan and
 * asks before the same losses.
 */
export async function release<Receipt>(client: Pick<McpClient, "call">, transferId: string, confirm: ConfirmSchema): Promise<{ plan: ReleasePlan; receipt: Receipt }> {
  const plan = await client.call<ReleasePlan>("release_plan", { transferId });
  const confirmation = plan.schema?.confirmation;
  if (plan.schema && confirmation && !await confirm(plan.schema)) throw new SchemaChangeDeclined(plan.schema);
  const receipt = await client.call<Receipt>("release_apply", { planId: plan.planId, ...(confirmation ? { confirmSchema: confirmation } : {}) });
  return { plan, receipt };
}

/** The project record project_create registers. */
export type ProjectIdentity = { id: string; name: string; slug: string; manifest: Record<string, unknown> };

/**
 * What a release uploads. `initialSignupPolicy` goes only with a first
 * release. `contactEmail` is where notices to the owner go.
 */
export type ReleaseContents = {
  project: object;
  visibility: unknown;
  artifact: object;
  initialSignupPolicy?: { mode: string; allowlist: string[] };
  contactEmail?: string;
};

/**
 * Publishes a project: project_create, the upload of what the release
 * carries, then release(). The editor and `konae publish` both publish
 * through here. `contents` runs after project_create, so it can read the
 * project that call registered. The upload is cancelled whatever happens,
 * and `cancelFailed` hears when that cancel fails.
 */
export async function publishProject<Receipt>(
  client: Pick<McpClient, "call" | "upload" | "cancel">,
  project: ProjectIdentity,
  contents: () => ReleaseContents | Promise<ReleaseContents>,
  confirm: ConfirmSchema,
  cancelFailed: (error: unknown) => void = () => {},
): Promise<{ plan: ReleasePlan; receipt: Receipt }> {
  await client.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const { project: sent, visibility, artifact, initialSignupPolicy, contactEmail } = await contents();
  const payload = { project: sent, visibility, artifact, ...(initialSignupPolicy ? { initialSignupPolicy } : {}), ...(contactEmail ? { contactEmail } : {}) };
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify(payload)), project.id);
  try {
    return await release<Receipt>(client, transfer.transferId, confirm);
  } finally {
    await client.cancel(transfer.transferId).catch(cancelFailed);
  }
}

/** The plan in lines for a terminal, with what it loses and then the statements. */
export function describeSchemaChange(change: SchemaChange): string[] {
  return [
    "This release changes the database and loses data:",
    ...change.losses.map((loss) => `  ${loss.sentence}`),
    "The Kernel runs, in one transaction:",
    ...change.statements.map((statement) => `  ${statement};`),
  ];
}
