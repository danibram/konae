import { sqlStatementTokens, type SqlToken } from "../lib/sql-tokens";

export type SqlKind = {
  read: boolean;
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
  if (first === "ALTER") return { read: false, danger: "ALTER TABLE changes the table structure outside schema.sql. Change schema.sql to match, so a new database gets the same table." };
  if (first === "CREATE") return { read: false, danger: "CREATE changes the database structure outside schema.sql. Add it to schema.sql too, so a new database gets it." };

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
  return { read: false, danger: null };
}

const reportingPragmas = new Set([
  "table_info", "table_xinfo", "table_list", "index_info", "index_xinfo", "index_list",
  "foreign_key_list", "foreign_key_check", "integrity_check", "quick_check",
]);

function pragmaReads(tokens: SqlToken[]): boolean {
  const outer = tokens[0].depth;
  const setter = tokens.findIndex((token) => token.kind === "symbol" && token.depth === outer && (token.text === "=" || token.text === "("));
  if (setter < 0) return true;
  if ((tokens[setter] as { text: string }).text === "=") return false;
  const name = tokens[setter - 1];
  return (name?.kind === "word" || name?.kind === "identifier") && reportingPragmas.has(name.text.toLowerCase());
}

function mainVerbAfterWith(tokens: SqlToken[]): number {
  const outer = tokens[0].depth;
  for (let index = 1; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (token.kind === "word" && token.depth === outer && mainVerbs.has(token.text.toUpperCase())) return index;
  }
  return -1;
}

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
