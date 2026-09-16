import { sqlTokens } from "../lib/sql-tokens";

type DeclaredAction = { name: string; kind: string; access: string; statement: string };

/**
 * The Auth panel's heading, read from the same actions as the rules under it.
 * It names who may run the list query and, when the query names a number, where
 * the list stops. With no list to describe it makes no claim.
 */
export function authHeading(actions: DeclaredAction[]): string {
  const queries = actions.filter((action) => action.kind === "query");
  const list = queries.find((action) => action.name.split(".").pop() === "list") ?? queries[0];
  if (!list) return "Choose who can open the app and who can run each action.";
  const who = list.access === "anonymous" ? "Anyone" : "A signed-in person";
  const verb = list.name.split(".").pop() === "list" ? "list the rows" : `run ${list.name}`;
  const limit = topLevelLimit(list.statement);
  const stops = limit === null ? "" : ` The list stops at ${limit} row${limit === 1 ? "" : "s"}.`;
  return `${who} can ${verb}.${stops}`;
}

/** The literal row count after a top-level LIMIT, or null when it is a parameter or missing. */
function topLevelLimit(statement: string): number | null {
  const top = sqlTokens(statement).filter((token) => token.depth === 0);
  const at = top.findIndex((token) => token.kind === "word" && token.text.toUpperCase() === "LIMIT");
  const value = at === -1 ? undefined : top[at + 1];
  return value && value.kind !== "string" && /^\d+$/.test(value.text) ? Number(value.text) : null;
}
