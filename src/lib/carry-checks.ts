// schema.sql may add CHECK constraints that a Database Model cannot state, and
// Drizzle rebuilds a table from the model alone (`CREATE TABLE __new_x`, copy,
// drop, rename). The statements a migration runs carry each CHECK over from the
// live table's SQL. kernel-rs/src/checks.rs does the same for a Deployment, and
// kernel-rs/src/check-carry-cases.json holds the cases both test suites run.
import { sqlTokens, type SqlToken } from "./sql-tokens";

export type CarriedMigration = { statements: string[] } | { error: string };

type Check = { owner: string | null; text: string; names: string[] };
type Table = { columns: string[]; checks: Check[]; close: SqlToken | null };

const constraintWords = ["CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"];

/**
 * The statements to run for `statements`, Drizzle's plan, against a database
 * whose tables `liveTable` reads. A rebuilt table gets the live table's CHECKs,
 * as written. A column's own CHECK leaves with the column. Any other CHECK that
 * names a column the plan removes refuses the whole migration. Drizzle's
 * `PRAGMA foreign_keys` lines are left out: inside the transaction a migration
 * runs in, SQLite ignores them.
 */
export function carryChecks(statements: string[], liveTable: (name: string) => string | null): CarriedMigration {
  const renames = new Map<string, string>();
  for (const statement of statements) {
    const tokens = sqlTokens(statement);
    if (isWord(tokens[0], "ALTER") && isWord(tokens[1], "TABLE") && isWord(tokens[3], "RENAME") && isWord(tokens[4], "TO") && tokens[2] && tokens[5]) {
      renames.set(nameOf(tokens[2]).toLowerCase(), nameOf(tokens[5]));
    }
  }
  const result: string[] = [];
  for (const statement of statements) {
    const tokens = sqlTokens(statement);
    if (isWord(tokens[0], "PRAGMA") && tokens[1] && nameOf(tokens[1]).toLowerCase() === "foreign_keys") continue;
    if (isWord(tokens[0], "CREATE") && isWord(tokens[1], "TABLE")) {
      const nameToken = tokens[isWord(tokens[2], "IF") ? 5 : 2];
      const target = nameToken ? renames.get(nameOf(nameToken).toLowerCase()) : undefined;
      const live = target ? liveTable(target) : null;
      if (target && live) {
        const before = readTable(live);
        const after = readTable(statement);
        const kept = after.columns.map((column) => column.toLowerCase());
        const carried: string[] = [];
        for (const check of before.checks) {
          if (check.owner && !kept.includes(check.owner.toLowerCase())) continue;
          const removed = removedName(check, before.columns, kept);
          if (removed) return { error: refusal(target, removed) };
          carried.push(check.text);
        }
        if (carried.length > 0 && after.close) {
          const head = statement.slice(0, after.close.start).trimEnd();
          result.push(`${head},\n\t${carried.join(",\n\t")}\n${statement.slice(after.close.start)}`);
          continue;
        }
      }
    }
    if (isWord(tokens[0], "ALTER") && isWord(tokens[1], "TABLE") && isWord(tokens[3], "DROP") && tokens[2]) {
      const columnToken = tokens[isWord(tokens[4], "COLUMN") ? 5 : 4];
      const table = nameOf(tokens[2]);
      const live = liveTable(table);
      if (columnToken && live) {
        const column = nameOf(columnToken).toLowerCase();
        const before = readTable(live);
        const kept = before.columns.map((name) => name.toLowerCase()).filter((name) => name !== column);
        for (const check of before.checks) {
          if (check.owner?.toLowerCase() === column) continue;
          const removed = removedName(check, before.columns, kept);
          if (removed) return { error: refusal(table, removed) };
        }
      }
    }
    result.push(statement);
  }
  return { statements: result };
}

function refusal(table: string, column: string): string {
  return `Table ${table} has a CHECK that names column ${column}, which this migration removes. Rebuild ${table} without that CHECK in the SQL console, or keep ${column}, then apply the migration again.`;
}

/** The first column of the old table that `check` names and the new table lacks, as the old table writes it. */
function removedName(check: Check, columns: string[], kept: string[]): string | null {
  for (const name of check.names) {
    const column = columns.find((candidate) => candidate.toLowerCase() === name.toLowerCase());
    if (column && !kept.includes(column.toLowerCase())) return column;
  }
  return null;
}

/** The columns and CHECKs of a CREATE TABLE statement, and the `)` that closes its definitions. */
function readTable(sql: string): Table {
  const tokens = sqlTokens(sql);
  const open = tokens.findIndex((token) => token.kind === "symbol" && token.text === "(" && token.depth === 0);
  const table: Table = { columns: [], checks: [], close: null };
  if (open === -1) return table;
  let definition: SqlToken[] = [];
  for (let index = open + 1; index < tokens.length; index++) {
    const token = tokens[index];
    const closes = token.kind === "symbol" && token.text === ")" && token.depth === 0;
    if (closes || (token.kind === "symbol" && token.text === "," && token.depth === 1)) {
      readDefinition(sql, definition, table);
      definition = [];
      if (closes) {
        table.close = token;
        break;
      }
      continue;
    }
    definition.push(token);
  }
  return table;
}

function readDefinition(sql: string, tokens: SqlToken[], table: Table) {
  const first = tokens[0];
  if (!first) return;
  const owner = first.kind === "word" && constraintWords.includes(first.text.toUpperCase()) ? null : nameOf(first);
  if (owner !== null) table.columns.push(owner);
  for (let index = 0; index < tokens.length; index++) {
    const open = tokens[index + 1];
    if (!isWord(tokens[index], "CHECK") || tokens[index].depth !== 1 || open?.kind !== "symbol" || open.text !== "(") continue;
    const close = tokens.findIndex((token, at) => at > index + 1 && token.kind === "symbol" && token.text === ")" && token.depth === 1);
    if (close === -1) return;
    const start = index >= 2 && isWord(tokens[index - 2], "CONSTRAINT") ? tokens[index - 2].start : tokens[index].start;
    const names = tokens.slice(index + 2, close).flatMap((token) => token.kind === "word" || token.kind === "identifier" ? [token.text] : []);
    table.checks.push({ owner, text: sql.slice(start, tokens[close].end), names });
    index = close;
  }
}

function isWord(token: SqlToken | undefined, word: string): boolean {
  return token?.kind === "word" && token.text.toUpperCase() === word;
}

function nameOf(token: SqlToken): string {
  return token.kind === "word" || token.kind === "identifier" ? token.text : "";
}
