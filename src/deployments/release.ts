import type { McpClient, Transfer } from "../mcp/client";
import { checkArtifact, checkRelease } from "../lib/size";
import { checkSourceFiles } from "../lib/source-rules";

/** What a schema change loses, as the Kernel words it. */
export type SchemaLoss = { table: string; column: string | null; sentence: string };

/**
 * The change `release_plan` derives from the app's live database to the one
 * schema.sql states (kernel-rs/kernel/src/schema_change.rs). `confirmation` is set
 * only when the change loses data, and release_apply then needs it back.
 */
export type SchemaChange = { statements: string[]; losses: SchemaLoss[]; confirmation: string | null };

/**
 * `schema` is absent when release_plan replays an import it already applied.
 * `hostname` and `url` name the generated address the release answers on; on a
 * first release (`firstRelease`) they are the name it will take, null when the
 * slug and its coded name are both taken.
 */
export type ReleasePlan = { planId: string; projectId: string; expectedRevision: number; schema?: SchemaChange; hostname?: string | null; url?: string | null; firstRelease?: boolean };

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
 * What release_plan takes. `transferId` is the upload of the release JSON and
 * `sourcesTransferId` the upload of the files of app/, which travel apart so
 * each upload stays under its own limit.
 */
export type ReleaseUploads = { transferId: string; sourcesTransferId?: string };

/**
 * Releases with release_plan, then release_apply. The editor and `konae publish`
 * both release through here, so each one shows the same plan and asks before
 * the same losses.
 */
export async function release<Receipt>(client: Pick<McpClient, "call">, uploads: ReleaseUploads, confirm: ConfirmSchema, onPlan?: (plan: ReleasePlan) => void): Promise<{ plan: ReleasePlan; receipt: Receipt }> {
  const plan = await client.call<ReleasePlan>("release_plan", uploads);
  onPlan?.(plan);
  const confirmation = plan.schema?.confirmation;
  if (plan.schema && confirmation && !await confirm(plan.schema)) throw new SchemaChangeDeclined(plan.schema);
  const receipt = await client.call<Receipt>("release_apply", { planId: plan.planId, ...(confirmation ? { confirmSchema: confirmation } : {}) });
  return { plan, receipt };
}

/** The project record project_create registers. */
export type ProjectIdentity = { id: string; name: string; slug: string; manifest: Record<string, unknown> };

/** A document that names its base64 files in `encodings`: the files of app/, or a Build Artifact. */
type CarriedFiles = { files: Record<string, string>; encodings?: Record<string, string> };

/**
 * What a release uploads. `project` holds the files of app/ beside its
 * identity. `initialSignupPolicy` goes only with a first release.
 * `contactEmail` is where notices to the owner go.
 */
export type ReleaseContents = {
  project: ProjectIdentity & CarriedFiles;
  visibility: unknown;
  artifact: CarriedFiles;
  initialSignupPolicy?: { mode: string; allowlist: string[] };
  contactEmail?: string;
};

/**
 * Publishes a project: project_create, the upload of the files of app/ and
 * the upload of the release JSON, then release(). The editor and `konae
 * publish` both publish through here. The sources, the artifact and the
 * release JSON meet the Kernel's limits before a byte travels, and a refusal
 * names the size and the limit. `contents` runs after project_create, so it
 * can read the project that call registered. Both uploads are cancelled
 * whatever happens, and `cancelFailed` hears when a cancel fails.
 */
export async function publishProject<Receipt>(
  client: Pick<McpClient, "call" | "upload" | "cancel">,
  project: ProjectIdentity,
  contents: () => ReleaseContents | Promise<ReleaseContents>,
  confirm: ConfirmSchema,
  cancelFailed: (error: unknown) => void = () => {},
  onPlan?: (plan: ReleasePlan) => void,
): Promise<{ plan: ReleasePlan; receipt: Receipt }> {
  await client.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const { project: sent, visibility, artifact, initialSignupPolicy, contactEmail } = await contents();
  const sources: CarriedFiles = sent.encodings && Object.keys(sent.encodings).length ? { files: sent.files, encodings: sent.encodings } : { files: sent.files };
  checkSourceFiles(sources);
  checkArtifact(artifact);
  const identity = { id: sent.id, name: sent.name, slug: sent.slug, manifest: sent.manifest };
  const payload = new TextEncoder().encode(JSON.stringify({ project: identity, visibility, artifact, ...(initialSignupPolicy ? { initialSignupPolicy } : {}), ...(contactEmail ? { contactEmail } : {}) }));
  checkRelease(payload);
  const transfers: Transfer[] = [];
  try {
    const sourcesTransfer = await client.upload(new TextEncoder().encode(JSON.stringify(sources)), project.id);
    transfers.push(sourcesTransfer);
    const releaseTransfer = await client.upload(payload, project.id);
    transfers.push(releaseTransfer);
    return await release<Receipt>(client, { transferId: releaseTransfer.transferId, sourcesTransferId: sourcesTransfer.transferId }, confirm, onPlan);
  } finally {
    for (const transfer of transfers) await client.cancel(transfer.transferId).catch(cancelFailed);
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
