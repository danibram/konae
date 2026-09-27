import { sqlTokens, type SqlToken } from "../lib/sql-tokens";

// The same checks, in the same order and with the same messages, as
// kernel-rs/src/sql_rules.rs `check_statement`. kernel-rs/src/sql-rule-cases.json
// holds the cases both test suites run, with the schema cases the Validation
// Gate refuses in src/agent/gate.ts.

const MUTATION_WORDS = new Set(["INSERT", "UPDATE", "DELETE", "REPLACE"]);
const STRUCTURE_WORDS = new Set(["CREATE", "ALTER", "DROP"]);

/** The first keyword, one statement, and no RETURNING on a mutation. */
export function assertActionStatement(name: string, kind: "query" | "mutation", statement: string) {
  const tokens = sqlTokens(statement);
  const first = tokens.find((token) => token.kind === "word");
  const word = first?.kind === "word" ? first.text.toUpperCase() : "";
  const startsRight = kind === "query" ? word === "SELECT" : MUTATION_WORDS.has(word);
  if (!startsRight) throw new Error(firstWordRefusal(name, kind, word));
  assertSingleStatement(name, statement);
  if (kind === "mutation" && returnsRows(tokens)) {
    throw new Error(`Action ${name} cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.`);
  }
}

function firstWordRefusal(name: string, kind: "query" | "mutation", word: string): string {
  const rule = kind === "query"
    ? `Action ${name} is a query, so it must start with SELECT.`
    : `Action ${name} is a mutation, so it must start with INSERT, UPDATE, DELETE or REPLACE.`;
  const hint = word === "WITH"
    ? kind === "query"
      ? " Move the WITH clause into a subquery, as in SELECT ... FROM (WITH ... SELECT ...)."
      : " Put the WITH clause after INSERT INTO <table> (<columns>), or inside a subquery."
    : STRUCTURE_WORDS.has(word) ? " Tables and indexes belong in schema.sql, and Littlebox runs no triggers or views." : "";
  return rule + hint;
}

/** A RETURNING keyword outside parentheses. `:returning`, `@returning` and `$returning` are parameters. */
function returnsRows(tokens: SqlToken[]): boolean {
  return tokens.some((token, index) => {
    const previous = tokens[index - 1];
    const parameter = previous?.kind === "symbol" && [":", "@", "$"].includes(previous.text);
    return token.kind === "word" && token.depth === 0 && token.text.toUpperCase() === "RETURNING" && !parameter;
  });
}

/** Anything after the first `;` other than comments and whitespace is a second statement. */
function assertSingleStatement(name: string, sql: string) {
  let quote: "'" | '"' | "`" | "]" | null = null;
  let terminated = false;
  for (let index = 0; index < sql.length; index += 1) {
    const char = sql[index];
    const next = sql[index + 1];
    if (!quote && char === "-" && next === "-") {
      index = sql.indexOf("\n", index + 2);
      if (index === -1) break;
      continue;
    }
    if (!quote && char === "/" && next === "*") {
      const end = sql.indexOf("*/", index + 2);
      if (end === -1) throw new Error("Unterminated SQL comment.");
      index = end + 1;
      continue;
    }
    if (!quote && (char === "'" || char === '"' || char === "`" || char === "[")) {
      if (terminated) throw new Error(`Action ${name} must contain one statement.`);
      quote = char === "[" ? "]" : char;
      continue;
    }
    if (quote && char === quote) {
      if (next === quote && quote !== "]") { index += 1; continue; }
      quote = null;
      continue;
    }
    if (!quote && char === ";") {
      if (terminated) throw new Error(`Action ${name} must contain one statement.`);
      terminated = true;
      continue;
    }
    if (terminated && !/\s/.test(char)) throw new Error(`Action ${name} must contain one statement.`);
  }
  if (quote) throw new Error("Unterminated SQL string.");
}
