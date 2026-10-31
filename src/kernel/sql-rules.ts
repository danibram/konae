import { sqlTokens, type SqlToken } from "../lib/sql-tokens";
import { refusal } from "./refusal";

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
  if (!startsRight) throw refusal(kind === "query" ? "query.start" : "mutation.start", firstWordRefusal(name, kind, word));
  assertSingleStatement(name, tokens);
  if (kind === "mutation" && returnsRows(tokens)) {
    throw refusal("mutation.returning", `Action ${name} cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.`);
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
    : STRUCTURE_WORDS.has(word) ? " Tables and indexes belong in schema.sql, and Konae runs no triggers or views." : "";
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

/**
 * A `;` token followed by any other token is a second statement, as the Kernel
 * reads it. Comments and whitespace are not tokens; a character at or above
 * U+0080, such as a no-break space, is part of a word, as SQLite reads it.
 */
function assertSingleStatement(name: string, tokens: SqlToken[]) {
  const end = tokens.findIndex((token) => token.kind === "symbol" && token.text === ";");
  if (end !== -1 && end + 1 < tokens.length) throw refusal("statement.several", `Action ${name} must contain one statement.`);
}
