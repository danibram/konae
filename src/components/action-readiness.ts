import { parseServerActionSource, routeCallers, type HttpBinding, type RouteCaller } from "../kernel/actions";
import { releaseRefusal, type ReleaseFacts } from "../agent/release-check";
import { ownerFixes } from "../kernel/refusal";
import { limitIndex, unboundedQueryReason } from "../lib/query-bound";
import { sqlTokens, type SqlToken } from "../lib/sql-tokens";
import type { RuntimeActionAccess } from "../deployments/manifest";

export type DescribedAction = {
  name: string; kind: string; access: RuntimeActionAccess; statement?: string; http?: HttpBinding; guard?: string;
  bindings?: ReadonlyArray<{ source: string }>;
};
type Visibility = "public" | "private";

export { ownerFixes };
export type { ReleaseFacts };

export const emptyActionProgram = '{"version":1,"actions":{}}';

export function deployReadiness(
  project: { files: Record<string, string | undefined> },
  facts: ReleaseFacts = {},
): { refusal: string | null; ownerActs: boolean; actions: DescribedAction[] } {
  try {
    const program = parseServerActionSource(project.files["actions.json"] ?? emptyActionProgram);
    const refusal = releaseRefusal(program, facts);
    return { refusal: refusal?.message ?? null, ownerActs: ownerFixes(refusal?.code), actions: Object.values(program.actions) };
  } catch (error) {
    return { refusal: error instanceof Error ? error.message : String(error), ownerActs: false, actions: [] };
  }
}

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
  return "It returns at most 1000 rows.";
}

export function actorFiltered(statement: string, bindings: ReadonlyArray<{ source: string }>): boolean {
  const actorAt = new Set(bindings.flatMap((binding, index) => binding.source === "actor" ? [index + 1] : []));
  if (actorAt.size === 0) return false;
  const tokens = sqlTokens(statement);
  const named = tokens.some((token, index) => {
    const next = tokens[index + 1];
    return token.kind === "symbol" && [":", "@", "$"].includes(token.text) && next !== undefined && next.start === token.end && (next.kind === "word" || next.kind === "identifier");
  });
  if (named) return false;
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
  const first = word(0);
  if (!["SELECT", "UPDATE", "DELETE"].includes(first)) return false;
  if (tokens.some((token, index) => index > 0 && token.kind === "word" && token.text.toUpperCase() === "SELECT")) return false;
  if (top.some((_, at) => ["UNION", "EXCEPT", "INTERSECT", "JOIN"].includes(word(at)))) return false;
  if (top.filter((_, at) => word(at) === "WHERE").length !== 1) return false;
  const tableAt = top.findIndex((_, at) => word(at) === "FROM");
  if (first === "UPDATE" && tableAt !== -1) return false;
  const whereAt = top.findIndex((_, at) => word(at) === "WHERE");
  if (tableAt !== -1 && top.slice(tableAt, whereAt).some(({ token }) => token.kind === "symbol" && token.text === ",")) return false;
  const aliases = new Set<string>();
  if (first === "SELECT" && tableAt !== -1) {
    let item: SqlToken[] = [];
    const close = () => {
      const words = item[0]?.kind === "word" && ["DISTINCT", "ALL"].includes(item[0].text.toUpperCase()) ? item.slice(1) : item;
      const last = words.at(-1);
      const before = words.at(-2);
      if (words.length >= 2 && last && (last.kind === "word" || last.kind === "identifier") && !(before?.kind === "symbol" && before.text === ".")) aliases.add(unquote(last.text));
      item = [];
    };
    for (const { token } of top.slice(1, tableAt)) {
      if (token.kind === "symbol" && token.text === ",") close(); else item.push(token);
    }
    close();
  }
  const from = whereAt + 1;
  let to = top.findIndex((_, at) => at >= from && ["GROUP", "ORDER", "LIMIT", "HAVING", "RETURNING", "WINDOW"].includes(word(at)));
  if (to === -1) to = top.length;
  const terms: string[] = [];
  const names = new Map<number, string>();
  for (let at = from; at < to; at += 1) {
    const { token, index } = top[at];
    if (token.kind === "word" && ["OR", "NOT", "BETWEEN", "CASE"].includes(token.text.toUpperCase())) return false;
    if (token.kind === "word" && token.text.toUpperCase() === "AND") terms.push("AND");
    else if (token.kind === "word" || token.kind === "identifier") {
      const dot = top[at + 1]?.token;
      const name = top[at + 2]?.token;
      if (dot?.kind === "symbol" && dot.text === "." && (name?.kind === "word" || name?.kind === "identifier")) at += 2;
      else names.set(terms.length, unquote(token.text));
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
  let start = 0;
  for (let at = 0; at <= terms.length; at += 1) {
    if (at < terms.length && terms[at] !== "AND") continue;
    const part = terms.slice(start, at).join(" ");
    const column = part === "column = actor" ? start : part === "actor = column" ? start + 2 : -1;
    if (column !== -1 && !aliases.has(names.get(column) ?? "")) return true;
    start = at + 1;
  }
  return false;
}

function unquote(text: string) {
  return text.replace(/^["`[]|["`\]]$/g, "").toLowerCase();
}

export function actionWho(access: string, visibility: Visibility): "Anyone" | "A signed-in person" {
  return access === "anonymous" && visibility === "public" ? "Anyone" : "A signed-in person";
}

function callerWords(callers: RouteCaller[]): string {
  const words = callers.map((caller) => caller === "signature" ? "a signed request" : caller === "key" ? "an API key" : "a signed-in person");
  return words.length === 1 ? words[0] : `${words.slice(0, -1).join(", ")} or ${words[words.length - 1]}`;
}

export function topLevelLimit(statement: string): number | null {
  const top = sqlTokens(statement).filter((token) => token.depth === 0);
  const at = limitIndex(top);
  const value = at === -1 ? undefined : top[at + 1];
  return value && value.kind !== "string" && /^\d+$/.test(value.text) ? Number(value.text) : null;
}

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

export function refusedSpot(source: string, refusal: string | null): { action: string; index: number; value?: { start: number; end: number } } | null {
  const action = refusal?.match(/(?:Action|server action:?) ([A-Za-z0-9_.-]*[A-Za-z0-9_-])/)?.[1];
  if (!action) return null;
  const key = source.search(new RegExp(`"${action.replace(/\./g, "\\.")}"\\s*:`));
  if (key < 0) return null;
  const after = key + action.length + 2;
  const next = source.slice(after).search(/"[A-Za-z0-9_.-]+"\s*:\s*\{/);
  const end = next < 0 ? source.length : after + next;
  const statement = source.indexOf('"statement"', key);
  if (statement < 0 || statement >= end) return { action, index: key };
  return { action, index: statement, ...stringValueAfter(source, statement + '"statement"'.length) };
}

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
