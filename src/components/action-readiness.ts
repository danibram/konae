import { parseServerActionSource, routeCallers, type HttpBinding, type RouteCaller } from "../kernel/actions";
import { releaseRefusal, type ReleaseFacts } from "../agent/release-check";
import { ownerFixes } from "../kernel/refusal";
import { limitIndex, unboundedQueryReason } from "../lib/query-bound";
import { sqlTokens } from "../lib/sql-tokens";
import type { RuntimeActionAccess } from "../deployments/manifest";

export type DescribedAction = {
  name: string; kind: string; access: RuntimeActionAccess; statement?: string; http?: HttpBinding; guard?: string;
  /** In parameter order, so binding i fills ?i+1. Only `source` is read here, to find the actor. */
  bindings?: ReadonlyArray<{ source: string }>;
};
type Visibility = "public" | "private";

export { ownerFixes };
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
    return { refusal: refusal?.message ?? null, ownerActs: ownerFixes(refusal?.code), actions: Object.values(program.actions) };
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
  if (action.kind === "call") return `${who} The app sends the request it describes and writes no row itself.`;
  const own = action.statement !== undefined && actorFiltered(action.statement, action.bindings ?? []);
  if (action.kind !== "query") return `${who}${own ? " It changes only their own rows." : ""}${guard ? `${guard}.` : ""}`;
  return `${who} ${queryRows(action.statement)}${own ? " It reads only their own rows." : ""}`;
}

/**
 * How many rows a query that passed the LIMIT rule hands back, from the same
 * reading the Validation Gate makes (src/lib/query-bound.ts): a literal LIMIT,
 * a total with no GROUP BY, or a select with no table.
 */
function queryRows(statement: string | undefined): string {
  if (statement === undefined) return "It returns at most 1000 rows.";
  const limit = topLevelLimit(statement);
  if (limit !== null) return `The list stops at ${limit} row${limit === 1 ? "" : "s"}.`;
  const top = sqlTokens(statement).filter((token) => token.depth === 0);
  if (limitIndex(top) === -1 && unboundedQueryReason(statement) === null) {
    const words = top.map((token) => token.kind === "word" ? token.text.toUpperCase() : "");
    if (!words.includes("FROM")) return "It returns one row.";
    const select = words.indexOf("SELECT");
    const columns = top.slice(select + 1, words.indexOf("FROM")).filter((token) => token.kind === "symbol" && token.text === ",").length + 1;
    return columns === 1 ? "It returns one number." : "It returns one row of totals.";
  }
  // A LIMIT taken from a binding still stops at 1000 rows, the Kernel's ceiling for every result.
  return "It returns at most 1000 rows.";
}

/**
 * True only when the statement's top-level WHERE requires a column to equal
 * the actor binding, as one of its AND-joined conditions, with no top-level
 * OR or NOT that could let other rows through. Anything else, an actor checked inside a subquery or EXISTS
 * included, is false, so a line never claims a row filter the SQL does not
 * prove. Parameters number as SQLite numbers them. ?N is N, and a bare ? is
 * one past the largest number so far.
 */
export function actorFiltered(statement: string, bindings: ReadonlyArray<{ source: string }>): boolean {
  const actorAt = new Set(bindings.flatMap((binding, index) => binding.source === "actor" ? [index + 1] : []));
  if (actorAt.size === 0) return false;
  const tokens = sqlTokens(statement);
  const parameter = new Map<number, number>();
  let largest = 0;
  tokens.forEach((token, index) => {
    if (token.kind !== "symbol" || token.text !== "?") return;
    const next = tokens[index + 1];
    const explicit = next && next.kind === "symbol" && next.start === token.end && /^\d+$/.test(next.text) ? Number(next.text) : null;
    const number = explicit ?? largest + 1;
    largest = Math.max(largest, number);
    parameter.set(index, number);
  });
  const top = tokens.map((token, index) => ({ token, index })).filter(({ token }) => token.depth === 0);
  const word = (at: number) => { const token = top[at]?.token; return token?.kind === "word" ? token.text.toUpperCase() : ""; };
  // One table read or changed by one statement. A join, a second table, a nested SELECT or an
  // UPDATE ... FROM can hand back rows the actor filter does not cover, so they claim nothing.
  const first = word(0);
  if (!["SELECT", "UPDATE", "DELETE"].includes(first)) return false;
  if (tokens.some((token, index) => index > 0 && token.kind === "word" && token.text.toUpperCase() === "SELECT")) return false;
  if (top.some((_, at) => ["UNION", "EXCEPT", "INTERSECT", "JOIN"].includes(word(at)))) return false;
  if (top.filter((_, at) => word(at) === "WHERE").length !== 1) return false;
  const tableAt = top.findIndex((_, at) => word(at) === "FROM");
  if (first === "UPDATE" && tableAt !== -1) return false;
  const whereAt = top.findIndex((_, at) => word(at) === "WHERE");
  if (tableAt !== -1 && top.slice(tableAt, whereAt).some(({ token }) => token.kind === "symbol" && token.text === ",")) return false;
  const from = whereAt + 1;
  let to = top.findIndex((_, at) => at >= from && ["GROUP", "ORDER", "LIMIT", "HAVING", "RETURNING", "WINDOW"].includes(word(at)));
  if (to === -1) to = top.length;
  // The clause as terms: AND, a column (qualified or not), the actor parameter, `=`, or anything else.
  const terms: string[] = [];
  for (let at = from; at < to; at += 1) {
    const { token, index } = top[at];
    // OR and NOT can let other rows through; BETWEEN and CASE carry an AND that joins no condition.
    if (token.kind === "word" && ["OR", "NOT", "BETWEEN", "CASE"].includes(token.text.toUpperCase())) return false;
    if (token.kind === "word" && token.text.toUpperCase() === "AND") terms.push("AND");
    else if (token.kind === "word" || token.kind === "identifier") {
      const dot = top[at + 1]?.token;
      const name = top[at + 2]?.token;
      if (dot?.kind === "symbol" && dot.text === "." && (name?.kind === "word" || name?.kind === "identifier")) at += 2;
      terms.push("column");
    } else if (token.kind === "symbol" && token.text === "?") {
      if (parameter.get(index) !== undefined && tokens[index + 1]?.start === token.end && /^\d+$/.test((tokens[index + 1] as { text?: string }).text ?? "")) at += 1;
      terms.push(actorAt.has(parameter.get(index) ?? -1) ? "actor" : "value");
    } else if (token.kind === "symbol" && token.text === "=") {
      const before = top[at - 1]?.token;
      const after = top[at + 1]?.token;
      const joined = (before?.kind === "symbol" && ["!", "<", ">", "="].includes(before.text) && before.end === token.start) || (after?.kind === "symbol" && after.text === "=" && after.start === token.end);
      terms.push(joined ? "other" : "=");
    } else terms.push("other");
  }
  // One AND-joined term must read `column = actor` or `actor = column` on its own.
  const parts = terms.join(" ").split("AND").map((part) => part.trim());
  return parts.some((part) => part === "column = actor" || part === "actor = column");
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
  return actionTitle(name);
}

/**
 * A person's words for an action, from the verb at the end of its name and the
 * words before it. A name proves nothing about whose rows the action reads, so
 * no title says "their own"; actionSentence says it when the SQL proves it.
 */
export function actionTitle(name: string): string {
  const parts = name.split(".");
  const verb = parts.pop() ?? name;
  const subject = parts.join(" ").replaceAll("_", " ");
  if (verb === "list") return subject ? `List ${subject}` : "List the rows";
  if (verb === "create") return "Add a row";
  if (verb === "toggle") return "Mark a row done";
  if (verb === "delete" || verb === "remove") return "Delete a row";
  if (verb === "update" || verb === "edit") return "Change a row";
  if (verb === "me") return subject ? `Read ${subject} for the signed-in person` : "Read for the signed-in person";
  if (verb === "send") return "Send a message";
  const words = verb.replaceAll("_", " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/**
 * Where in actions.json a refusal points: the action it names, and inside it
 * the statement when the action has one. Null when the refusal names no action
 * the file holds, such as a file that is not JSON.
 */
export function refusedSpot(source: string, refusal: string | null): { action: string; index: number; value?: { start: number; end: number } } | null {
  const action = refusal?.match(/(?:Action|server action:?) ([A-Za-z0-9_.-]*[A-Za-z0-9_-])/)?.[1];
  if (!action) return null;
  const key = source.search(new RegExp(`"${action.replace(/\./g, "\\.")}"\\s*:`));
  if (key < 0) return null;
  // The statement counts only while it belongs to this action, before the next one opens.
  const after = key + action.length + 2;
  const next = source.slice(after).search(/"[A-Za-z0-9_.-]+"\s*:\s*\{/);
  const end = next < 0 ? source.length : after + next;
  const statement = source.indexOf('"statement"', key);
  if (statement < 0 || statement >= end) return { action, index: key };
  return { action, index: statement, ...stringValueAfter(source, statement + '"statement"'.length) };
}

/** The span of the JSON string that follows `"key":` at `from`, quotes excluded, when the file has one there. */
function stringValueAfter(source: string, from: number): { value?: { start: number; end: number } } {
  const open = /^\s*:\s*"/.exec(source.slice(from));
  if (!open) return {};
  const start = from + open[0].length;
  for (let at = start; at < source.length; at += 1) {
    if (source[at] === "\\") at += 1;
    else if (source[at] === '"') return { value: { start, end: at } };
    else if (source[at] === "\n") return {};
  }
  return {};
}
