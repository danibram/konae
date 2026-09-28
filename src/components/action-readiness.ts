import { parseServerActionSource } from "../kernel/actions";
import { sqlTokens } from "../lib/sql-tokens";
import type { RuntimeActionAccess } from "../deployments/manifest";

export type DescribedAction = { name: string; kind: string; access: RuntimeActionAccess; statement?: string };

/**
 * What the Deploy dialog may say before a publish. actions.json goes through
 * the parser both doors share, so a refusal here is the sentence the Kernel
 * answers at release_plan, and a query that parsed already names its LIMIT.
 */
export function deployReadiness(project: { files: Record<string, string | undefined>; manifest: { runtime: { database: { actions: DescribedAction[] } } } }):
  { refusal: string | null; actions: DescribedAction[] } {
  try {
    return { refusal: null, actions: Object.values(parseServerActionSource(project.files["actions.json"] ?? "").actions) };
  } catch (error) {
    return { refusal: error instanceof Error ? error.message : String(error), actions: project.manifest.runtime.database.actions };
  }
}

/** One line for an action in the Deploy dialog, with the row count its query names. */
export function actionSentence(action: DescribedAction): string {
  const who = action.access === "anonymous" ? "Anyone" : "A signed-in person";
  if (action.kind === "signal") return `${who} can run it. It records a Signal and writes no row.`;
  if (action.kind !== "query") return `${who} can run it.`;
  const limit = action.statement === undefined ? null : topLevelLimit(action.statement);
  if (limit !== null) return `${who} can run it. The list stops at ${limit} row${limit === 1 ? "" : "s"}.`;
  // A query that parsed and names no number is a total, reads no table, or takes its LIMIT from a binding.
  return `${who} can run it. The Kernel stops it at 1000 rows.`;
}

/** The literal row count after a top-level LIMIT, or null when it is a parameter or missing. */
export function topLevelLimit(statement: string): number | null {
  const top = sqlTokens(statement).filter((token) => token.depth === 0);
  const at = top.findIndex((token) => token.kind === "word" && token.text.toUpperCase() === "LIMIT");
  const value = at === -1 ? undefined : top[at + 1];
  return value && value.kind !== "string" && /^\d+$/.test(value.text) ? Number(value.text) : null;
}

/** The readout above actions.json in the file editor: the parser's refusal, or one line per action. */
export function actionReadout(source: string): { refusal: string | null; lines: string[] } {
  try {
    const actions = Object.values(parseServerActionSource(source).actions);
    return { refusal: null, lines: actions.map((action) => `${actionDoes(action.name)}. ${actionSentence(action)}`) };
  } catch (error) {
    return { refusal: error instanceof Error ? error.message : String(error), lines: [] };
  }
}

function actionDoes(name: string): string {
  const verb = name.split(".").pop();
  return verb === "list" ? "Lists the rows" : verb === "create" ? "Adds a row" : verb === "toggle" ? "Marks a row done" : name;
}
