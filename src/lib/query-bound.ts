import { sqlTokens, type SqlToken } from "./sql-tokens";

const aggregates = new Set(["AVG", "COUNT", "GROUP_CONCAT", "MAX", "MIN", "SUM", "TOTAL"]);

/** Same sentence in Describe, Files, and Deploy when a list has no LIMIT. */
export const LIMIT_REFUSE_SENTENCE =
  "A list that reads rows needs a LIMIT. This check refused a query without one.";

/** Breaks a statement before the clauses a person has to see, including LIMIT. */
export function presentStatement(sql: string): string {
  return sql.replace(/\s+(FROM|WHERE|GROUP BY|HAVING|ORDER BY|LIMIT)\b/gi, "\n$1");
}

/**
 * Why a query action may not ship, or null when it may.
 * A LIMIT counts only at the top level. One inside a subquery, a string or a
 * comment does not bound the rows the caller receives. A query with no FROM
 * returns one row. A query whose select list is only totals, with no GROUP BY,
 * returns one row. Everything else that reads a table has to name a LIMIT.
 * The Kernel's `unbounded_query_reason` repeats this so an exported app refuses
 * the same statements.
 */

export function unboundedQueryReason(sql: string): string | null {
  const tokens = sqlTokens(sql);
  const top = tokens.filter((token) => token.depth === 0);
  if (hasWord(top, "LIMIT")) return null;

  if (top.some((token) => token.kind === "word" && ["EXCEPT", "INTERSECT", "UNION"].includes(token.text.toUpperCase()))) {
    return "combines more than one query and has no LIMIT on the whole result.";
  }
  if (tokens.some((token) => token.kind === "word" && token.text.toUpperCase() === "OVER")) {
    return "uses OVER, which keeps one result row per stored row, and has no LIMIT.";
  }

  const from = indexOfWord(top, "FROM");
  if (from === -1) return null;

  for (let index = 0; index < top.length - 1; index += 1) {
    const token = top[index];
    const next = top[index + 1];
    if (token.kind === "word" && token.text.toUpperCase() === "GROUP" && next.kind === "word" && next.text.toUpperCase() === "BY") {
      return "groups rows and has no LIMIT, so the result can grow with the data.";
    }
  }

  const select = indexOfWord(top, "SELECT");
  if (select === -1 || !aggregateSelectList(top.slice(select + 1, from))) {
    return "reads rows and has no LIMIT.";
  }
  return null;
}

function aggregateSelectList(list: SqlToken[]): boolean {
  let index = 0;
  if (list[0]?.kind === "word" && ["ALL", "DISTINCT"].includes(list[0].text.toUpperCase())) index = 1;
  if (index >= list.length) return false;

  let expressions = 0;
  while (index < list.length) {
    const current = list[index];
    if (current.kind === "symbol" && current.text === ",") {
      index += 1;
      continue;
    }
    const name = list[index];
    const open = list[index + 1];
    if (name?.kind !== "word" || !aggregates.has(name.text.toUpperCase()) || open?.kind !== "symbol" || open.text !== "(") {
      return false;
    }
    index += 2;
    const close = list.findIndex((token, at) => at >= index && token.kind === "symbol" && token.text === ")");
    if (close === -1) return false;
    index = close + 1;
    const after = list[index];
    if (after?.kind === "word" && after.text.toUpperCase() === "AS") {
      const alias = list[index + 1];
      if (alias?.kind !== "word" && alias?.kind !== "identifier") return false;
      index += 2;
    } else if (after?.kind === "word" || after?.kind === "identifier") {
      index += 1;
    }
    expressions += 1;
    const next = list[index];
    if (index < list.length && !(next.kind === "symbol" && next.text === ",")) return false;
  }
  return expressions > 0;
}

function hasWord(tokens: SqlToken[], word: string): boolean {
  return indexOfWord(tokens, word) !== -1;
}

function indexOfWord(tokens: SqlToken[], word: string): number {
  return tokens.findIndex((token) => token.kind === "word" && token.text.toUpperCase() === word);
}
