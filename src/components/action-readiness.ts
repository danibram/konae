import { parseServerActionSource, routeCallers, type HttpBinding, type RouteCaller } from "../kernel/actions";
import { refusalNeedsSecret, releaseRefusal, type ReleaseFacts } from "../agent/release-check";
import { limitIndex } from "../lib/query-bound";
import { sqlTokens } from "../lib/sql-tokens";
import type { RuntimeActionAccess } from "../deployments/manifest";

export type DescribedAction = { name: string; kind: string; access: RuntimeActionAccess; statement?: string; http?: HttpBinding; guard?: string };
type Visibility = "public" | "private";

export { refusalNeedsSecret };
export type { ReleaseFacts };

/**
 * What the Deploy dialog may say before a publish. actions.json goes through
 * the parser both doors share, so a refusal here is the sentence the Kernel
 * answers at release_plan, and a query that parsed already names its LIMIT.
 * With the project's secret names and the installation's hosts, the release
 * check the Validation Gate runs adds the sentences the Kernel says only at
 * publish. `ownerActs` is true when the owner, not an edit, clears the refusal.
 */
export function deployReadiness(
  project: { files: Record<string, string | undefined>; manifest: { runtime: { database: { actions: DescribedAction[] } } } },
  facts: ReleaseFacts = {},
): { refusal: string | null; ownerActs: boolean; actions: DescribedAction[] } {
  try {
    const program = parseServerActionSource(project.files["actions.json"] ?? "");
    const refusal = releaseRefusal(program, facts);
    return { refusal, ownerActs: refusal !== null && refusalNeedsSecret(refusal), actions: Object.values(program.actions) };
  } catch (error) {
    return { refusal: error instanceof Error ? error.message : String(error), ownerActs: false, actions: project.manifest.runtime.database.actions };
  }
}

/**
 * Every secret actions.json names, each with the actions that use it: a
 * route's signature and a call's auth. A name the project does not have
 * refuses the publish.
 */
export function namedSecrets(source: string | undefined): Map<string, { actions: string[]; route: boolean }> {
  const named = new Map<string, { actions: string[]; route: boolean }>();
  let program;
  try {
    program = parseServerActionSource(source ?? "");
  } catch {
    return named;
  }
  for (const name of Object.keys(program.actions).sort()) {
    const action = program.actions[name];
    const auth = action.call?.auth;
    const uses: [string | undefined, boolean][] = [[action.http?.signature?.secret, true], [auth ? ("secret" in auth ? auth.secret : auth.key) : undefined, false]];
    for (const [secret, route] of uses) {
      if (!secret) continue;
      const entry = named.get(secret) ?? { actions: [], route: false };
      if (!entry.actions.includes(name)) entry.actions.push(name);
      entry.route ||= route;
      named.set(secret, entry);
    }
  }
  return named;
}

/**
 * One line for an action: who may run it on a Deployment of this visibility,
 * where it stops, and what its guard answers. On a private Deployment even an
 * anonymous action needs a session, and an action whose route does not answer
 * anyone runs only on that route, as `routeCallers` decides for both doors.
 */
export function actionSentence(action: DescribedAction, visibility: Visibility): string {
  const only = action.http ? routeCallers(action.http, visibility) : null;
  const route = action.http ? `${action.http.method} ${action.http.path}` : "";
  const who = only
    ? `Only its route, ${route}, runs it, for ${callerWords(only)}.`
    : `${actionWho(action.access, visibility)} can run it.${action.http ? ` It also answers on ${route}.` : ""}`;
  const guard = action.guard ? ` When no row changes it refuses with "${action.guard}"` : "";
  if (action.kind === "signal") return `${who} It records a Signal and writes no row.`;
  if (action.kind === "call") return `${who} The Kernel sends the request it describes and writes no row itself.`;
  if (action.kind !== "query") return `${who}${guard ? `${guard}.` : ""}`;
  const limit = action.statement === undefined ? null : topLevelLimit(action.statement);
  if (limit !== null) return `${who} The list stops at ${limit} row${limit === 1 ? "" : "s"}.`;
  // A query that parsed and names no number is a total, reads no table, or takes its LIMIT from a binding.
  return `${who} The Kernel stops it at 1000 rows.`;
}

/** Who may call an action that the runtime endpoint answers. */
export function actionWho(access: string, visibility: Visibility): "Anyone" | "A signed-in person" {
  return access === "anonymous" && visibility === "public" ? "Anyone" : "A signed-in person";
}

function callerWords(callers: RouteCaller[]): string {
  const words = callers.map((caller) => caller === "signature" ? "a signed request" : caller === "key" ? "an API key" : "a signed-in person");
  return words.length === 1 ? words[0] : `${words.slice(0, -1).join(", ")} or ${words[words.length - 1]}`;
}

/** The literal row count after a top-level LIMIT, or null when it is a parameter or missing. */
export function topLevelLimit(statement: string): number | null {
  const top = sqlTokens(statement).filter((token) => token.depth === 0);
  const at = limitIndex(top);
  const value = at === -1 ? undefined : top[at + 1];
  return value && value.kind !== "string" && /^\d+$/.test(value.text) ? Number(value.text) : null;
}

/** The readout above actions.json in the file editor: the parser's refusal, or one line per action. */
export function actionReadout(source: string, visibility: Visibility): { refusal: string | null; lines: string[] } {
  try {
    const actions = Object.values(parseServerActionSource(source).actions);
    return { refusal: null, lines: actions.map((action) => `${actionDoes(action.name)}. ${actionSentence(action, visibility)}`) };
  } catch (error) {
    return { refusal: error instanceof Error ? error.message : String(error), lines: [] };
  }
}

function actionDoes(name: string): string {
  const verb = name.split(".").pop();
  return verb === "list" ? "Lists the rows" : verb === "create" ? "Adds a row" : verb === "toggle" ? "Marks a row done" : name;
}
