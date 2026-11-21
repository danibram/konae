import { parseFunctions, validateServerArtifact, type ServerArtifact } from "../kernel/functions";
import type { McpClient, Transfer } from "../mcp/client";
import { checkArtifact, checkRelease, checkSourceUpload } from "../lib/size";
import { checkSourceFiles } from "../lib/source-rules";
import type { SourceFiles } from "../lib/source-files";
import { refusal } from "../kernel/refusal";
import { buildProjectEntries } from "../lib/build-project";

export type SchemaLoss = { table: string; column: string | null; sentence: string };

export type SchemaTransformation = {
  id: string;
  definitionSha256: string;
  before: string;
  after: string;
  statements: string[];
  maxRows: number;
  affectedRows: number;
  baselineSkipped: boolean;
};

export type SchemaChange = { statements: string[]; losses: SchemaLoss[]; confirmation: string | null; transformations?: SchemaTransformation[] };

export type ReleasePlan = { planId: string; projectId: string; expectedRevision: number; expectedSourcesRevision: number; expiresAt: number; schema?: SchemaChange; hostname?: string | null; url?: string | null; firstRelease?: boolean };

export type ConfirmSchema = (change: SchemaChange) => Promise<boolean>;

export class SchemaChangeDeclined extends Error {
  constructor(readonly change: SchemaChange) {
    const effects = [
      ...change.losses.map((loss) => loss.sentence),
      ...(change.transformations ?? []).filter((item) => !item.baselineSkipped).map((item) => `Transition ${item.id} would change ${item.affectedRows} rows, with a limit of ${item.maxRows}.`),
    ];
    super(`Nothing was published. ${effects.join(" ")} Confirm the plan to apply it.`);
    this.name = "SchemaChangeDeclined";
  }
}

export type ReleaseUploads = { transferId: string; sourcesTransferId?: string; observedSourcesRevision?: number };

export function sourceRevisionConflict(observed: number) {
  return refusal("sources.revision-conflict", `Sources changed since revision ${observed}. Run konae pull --dir <folder>, then publish again.`);
}

export async function release<Receipt>(client: Pick<McpClient, "call">, uploads: ReleaseUploads, confirm: ConfirmSchema, onPlan?: (plan: ReleasePlan) => void): Promise<{ plan: ReleasePlan; receipt: Receipt }> {
  const plan = await planRelease(client, uploads, onPlan);
  const confirmation = plan.schema?.confirmation;
  if (plan.schema && confirmation && !await confirm(plan.schema)) throw new SchemaChangeDeclined(plan.schema);
  const receipt = await client.call<Receipt>("release_apply", { planId: plan.planId, ...(confirmation ? { confirmSchema: confirmation } : {}) });
  return { plan, receipt };
}

export async function planRelease(client: Pick<McpClient, "call">, uploads: ReleaseUploads, onPlan?: (plan: ReleasePlan) => void): Promise<ReleasePlan> {
  const plan = await client.call<ReleasePlan>("release_plan", uploads);
  onPlan?.(plan);
  return plan;
}

export type ProjectIdentity = { id: string; name: string; slug: string; manifest: Record<string, unknown> };

type CarriedFiles = { files: Record<string, string>; encodings?: Record<string, string> };

export type ReleaseContents = {
  project: ProjectIdentity & SourceFiles & { directories?: string[] };
  visibility: unknown;
  artifact: CarriedFiles & { serverArtifact?: ServerArtifact };
  serverArtifact?: ServerArtifact;
  initialSignupPolicy?: { mode: string; allowlist: string[] };
  initialModules?: Record<string, unknown>;
  contactEmail?: string;
};

export function publishProject<Receipt>(client: Pick<McpClient, "call" | "upload" | "cancel">, project: ProjectIdentity, contents: () => ReleaseContents | Promise<ReleaseContents>, confirm: ConfirmSchema, cancelFailed?: (error: unknown) => void, onPlan?: (plan: ReleasePlan) => void, observedSourcesRevision?: number | (() => number | undefined)): Promise<{ plan: ReleasePlan; receipt: Receipt }>;
export function publishProject(client: Pick<McpClient, "call" | "upload" | "cancel">, project: ProjectIdentity, contents: () => ReleaseContents | Promise<ReleaseContents>, confirm: ConfirmSchema, cancelFailed: (error: unknown) => void, onPlan: ((plan: ReleasePlan) => void) | undefined, observedSourcesRevision: number | (() => number | undefined) | undefined, apply: false, registerProject: false): Promise<{ plan: ReleasePlan }>;
export function publishProject<Receipt>(client: Pick<McpClient, "call" | "upload" | "cancel">, project: ProjectIdentity, contents: () => ReleaseContents | Promise<ReleaseContents>, confirm: ConfirmSchema, cancelFailed: (error: unknown) => void, onPlan: ((plan: ReleasePlan) => void) | undefined, observedSourcesRevision: number | (() => number | undefined) | undefined, apply: boolean, registerProject?: boolean): Promise<{ plan: ReleasePlan; receipt?: Receipt }>;
export async function publishProject<Receipt>(
  client: Pick<McpClient, "call" | "upload" | "cancel">,
  project: ProjectIdentity,
  contents: () => ReleaseContents | Promise<ReleaseContents>,
  confirm: ConfirmSchema,
  cancelFailed: (error: unknown) => void = () => {},
  onPlan?: (plan: ReleasePlan) => void,
  observedSourcesRevision?: number | (() => number | undefined),
  apply = true,
  registerProject = true,
): Promise<{ plan: ReleasePlan; receipt?: Receipt }> {
  if (registerProject) await client.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const { project: sent, visibility, artifact: built, serverArtifact: suppliedServer, initialSignupPolicy, initialModules, contactEmail } = await contents();
  const { serverArtifact: builtServer, ...artifact } = built;
  const serverArtifact = suppliedServer ?? builtServer;
  validateServerArtifact(sent.files["functions.json"] === undefined ? undefined : parseFunctions(sent.files["functions.json"]), serverArtifact);
  const sources: SourceFiles & { directories: string[] } = {
    files: sent.files,
    ...(sent.encodings && Object.keys(sent.encodings).length ? { encodings: sent.encodings } : {}),
    directories: sent.directories ?? [],
  };
  checkSourceFiles(sources);
  buildProjectEntries(sent);
  checkArtifact(artifact);
  const identity = { id: sent.id, name: sent.name, slug: sent.slug, manifest: sent.manifest };
  const payload = new TextEncoder().encode(JSON.stringify({ project: identity, visibility, artifact, ...(serverArtifact ? { serverArtifact } : {}), ...(initialSignupPolicy ? { initialSignupPolicy } : {}), ...(initialModules ? { initialModules } : {}), ...(contactEmail ? { contactEmail } : {}) }));
  checkRelease(payload);
  const sourceBytes = new TextEncoder().encode(JSON.stringify(sources));
  checkSourceUpload(sourceBytes.length);
  const transfers: Transfer[] = [];
  try {
    const sourcesTransfer = await client.upload(sourceBytes, project.id);
    transfers.push(sourcesTransfer);
    const releaseTransfer = await client.upload(payload, project.id);
    transfers.push(releaseTransfer);
    const observed = typeof observedSourcesRevision === "function" ? observedSourcesRevision() : observedSourcesRevision;
    const uploads = { transferId: releaseTransfer.transferId, sourcesTransferId: sourcesTransfer.transferId, observedSourcesRevision: observed };
    if (!apply) return { plan: await planRelease(client, uploads, onPlan) };
    return await release<Receipt>(client, uploads, confirm, onPlan);
  } finally {
    for (const transfer of transfers) await client.cancel(transfer.transferId).catch(cancelFailed);
  }
}

export function describeSchemaChange(change: SchemaChange): string[] {
  const lines: string[] = [];
  if (change.losses.length) lines.push("This release changes the database and loses data:", ...change.losses.map((loss) => `  ${loss.sentence}`));
  if (change.transformations?.length) {
    lines.push("The Kernel applies these data transformations:");
    for (const transition of change.transformations) {
      const baseline = transition.baselineSkipped ? " SQL is skipped for the fresh database." : "";
      lines.push(`  ${transition.id} changes ${transition.affectedRows} ${transition.affectedRows === 1 ? "row" : "rows"} (limit ${transition.maxRows}).${baseline}`);
      lines.push(...transition.statements.map((statement) => `    ${statement};`));
    }
  }
  if (change.statements.length) {
    lines.push("The Kernel runs these schema statements in one transaction:", ...change.statements.map((statement) => `  ${statement};`));
  }
  return lines;
}
