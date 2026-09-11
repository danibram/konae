import { sqlStatementTokens, type SqlToken } from "../lib/sql-tokens";

/**
 * Just enough SQL reading for the console to know whether it is about to run a
 * read, a write, or a statement that destroys data or structure. Each statement
 * of a script is classified on its own, from tokens that already exclude
 * comments and string literals, so a keyword or a `;` inside a value never counts.
 */
export type SqlKind = {
  /** Every statement only reads. A statement the console cannot read counts as a write. */
  read: boolean;
  /** Why the owner should confirm first, or null. */
  danger: string | null;
};

const mainVerbs = new Set(["SELECT", "VALUES", "INSERT", "REPLACE", "UPDATE", "DELETE"]);

export function classifySql(sql: string): SqlKind {
  const statements = sqlStatementTokens(sql);
  let read = statements.length > 0;
  const reasons: string[] = [];

  for (const tokens of statements) {
    const verdict = classifyStatement(tokens);
    if (!verdict.read) read = false;
    if (verdict.danger && !reasons.includes(verdict.danger)) reasons.push(verdict.danger);
  }

  return { read, danger: reasons.length > 0 ? reasons.join(" ") : null };
}

function classifyStatement(tokens: SqlToken[]): { read: boolean; danger: string | null } {
  const words = tokens.filter((token) => token.kind === "word").map((token) => token.text.toUpperCase());
  const first = words[0];
  if (!first) return { read: false, danger: null };

  if (first === "EXPLAIN") return { read: true, danger: null };
  if (first === "PRAGMA") return { read: pragmaReads(tokens), danger: null };
  if (first === "DROP") {
    const kind = `DROP ${words[1] ?? ""}`.trim();
    const name = objectName(tokens, words[2] === "IF" ? 4 : 2);
    return { read: false, danger: `${kind} deletes ${name ?? "it"} and everything in it.` };
  }
  if (first === "ALTER") return { read: false, danger: "ALTER TABLE changes the table structure outside schema.ts." };
  if (first === "CREATE") return { read: false, danger: "CREATE changes the database structure outside schema.ts." };

  // A leading WITH only names subqueries; the statement does what its main verb does.
  const main = first === "WITH" ? mainVerbAfterWith(tokens) : tokens.findIndex((token) => token.kind === "word");
  if (main < 0) return { read: false, danger: null };
  const verb = (tokens[main] as { text: string }).text.toUpperCase();
  if (verb === "SELECT" || verb === "VALUES") return { read: true, danger: null };
  if (verb === "DELETE" || verb === "UPDATE") {
    const depth = tokens[main].depth;
    const filtered = tokens.slice(main + 1).some((token) => token.kind === "word" && token.depth === depth && token.text.toUpperCase() === "WHERE");
    if (!filtered) {
      return {
        read: false,
        danger: verb === "DELETE" ? "DELETE without WHERE removes every row in the table." : "UPDATE without WHERE changes every row in the table.",
      };
    }
  }
  // INSERT, REPLACE, a filtered DELETE or UPDATE, and anything unrecognised: a write the studio must refresh after.
  return { read: false, danger: null };
}

/**
 * Pragmas whose `name(argument)` form only reports. For every other pragma the
 * argument form sets a value just as `name = value` does: `PRAGMA
 * user_version(5)` writes.
 */
const reportingPragmas = new Set([
  "table_info", "table_xinfo", "table_list", "index_info", "index_xinfo", "index_list",
  "foreign_key_list", "foreign_key_check", "integrity_check", "quick_check",
]);

/** Whether a PRAGMA only reads: no `=`, and no top-level `(` unless the pragma only reports. */
function pragmaReads(tokens: SqlToken[]): boolean {
  const outer = tokens[0].depth;
  const setter = tokens.findIndex((token) => token.kind === "symbol" && token.depth === outer && (token.text === "=" || token.text === "("));
  if (setter < 0) return true;
  if ((tokens[setter] as { text: string }).text === "=") return false;
  // The name is the word right before the "(", after any `schema.` prefix.
  const name = tokens[setter - 1];
  return (name?.kind === "word" || name?.kind === "identifier") && reportingPragmas.has(name.text.toLowerCase());
}

/**
 * The index of the statement's main verb after `WITH [RECURSIVE] name AS (...), ...`,
 * or -1. The CTE bodies sit inside parentheses, so the main verb is the first
 * verb back at the outer depth.
 */
function mainVerbAfterWith(tokens: SqlToken[]): number {
  const outer = tokens[0].depth;
  for (let index = 1; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (token.kind === "word" && token.depth === outer && mainVerbs.has(token.text.toUpperCase())) return index;
  }
  return -1;
}

/** The object a DROP names, counting bare words and quoted identifiers, and skipping a schema prefix such as `main.`. */
function objectName(tokens: SqlToken[], wordIndex: number): string | null {
  let seen = 0;
  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (token.kind !== "word" && token.kind !== "identifier") continue;
    if (seen === wordIndex) {
      const dot = tokens[index + 1];
      const after = tokens[index + 2];
      if (dot?.kind === "symbol" && dot.text === "." && after && (after.kind === "word" || after.kind === "identifier")) return after.text;
      return token.text;
    }
    seen += 1;
  }
  return null;
}

/** Table names declared with `sqliteTable("name", …)` in a Drizzle model. */
export function modelTableNames(source: string): string[] {
  return [...source.matchAll(/sqliteTable\(\s*["'`]([^"'`]+)["'`]/g)].map((match) => match[1]);
}
