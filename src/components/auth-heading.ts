import { actionWho, topLevelLimit } from "./action-readiness";

type DeclaredAction = { name: string; kind: string; access: string; statement: string };

/**
 * The Auth panel's heading, read from the same actions as the rules under it.
 * It names who may run the list query on a Deployment of this visibility and,
 * when the query names a number, where the list stops. With no list to describe it makes no claim.
 */
export function authHeading(actions: DeclaredAction[], visibility: "public" | "private"): string {
  const queries = actions.filter((action) => action.kind === "query");
  const list = queries.find((action) => action.name.split(".").pop() === "list") ?? queries[0];
  if (!list) return "Choose who can open the app and who can run each action.";
  const who = actionWho(list.access, visibility);
  const verb = list.name.split(".").pop() === "list" ? "list the rows" : `run ${list.name}`;
  const limit = topLevelLimit(list.statement);
  const stops = limit === null ? "" : ` The list stops at ${limit} row${limit === 1 ? "" : "s"}.`;
  return `${who} can ${verb}.${stops}`;
}
