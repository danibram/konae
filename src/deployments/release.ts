import type { McpClient } from "../mcp/client";

/** What a schema change loses, as the Kernel words it. */
export type SchemaLoss = { table: string; column: string | null; sentence: string };

/**
 * The change `release_plan` derives from the app's live database to the one
 * schema.sql states (kernel-rs/src/schema_change.rs). `confirmation` is set
 * only when the change loses data, and release_apply then needs it back.
 */
export type SchemaChange = { statements: string[]; losses: SchemaLoss[]; confirmation: string | null };

/** `schema` is absent when release_plan replays a package import it already applied. */
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

/** The plan in lines for a terminal, with what it loses and then the statements. */
export function describeSchemaChange(change: SchemaChange): string[] {
  return [
    "This release changes the database and loses data:",
    ...change.losses.map((loss) => `  ${loss.sentence}`),
    "The Kernel runs, in one transaction:",
    ...change.statements.map((statement) => `  ${statement};`),
  ];
}
