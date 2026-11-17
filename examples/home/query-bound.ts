import { sqlTokens, type SqlToken } from "./sql-tokens";

const aggregates = new Set(["AVG", "COUNT", "GROUP_CONCAT", "MAX", "MIN", "SUM", "TOTAL"]);

export function presentStatement(sql: string): string {
  return sql.replace(/\s+(FROM|WHERE|GROUP BY|HAVING|ORDER BY|LIMIT)\b/gi, "\n$1");
}

export function limitIndex(top: SqlToken[]): number {
  return top.findIndex((token, index) => {
    const previous = top[index - 1];
    const parameter = previous?.kind === "symbol" && [":", "@", "$"].includes(previous.text);
    return token.kind === "word" && token.depth === 0 && token.text.toUpperCase() === "LIMIT" && !parameter;
  });
}


export function unboundedQueryReason(sql: string): string | null {
  const tokens = sqlTokens(sql);
  const top = tokens.filter((token) => token.depth === 0);
  if (limitIndex(top) !== -1) return null;

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
  if (select === -1 || !aggregateSelectList(tokens.slice(tokens.indexOf(top[select]) + 1, tokens.indexOf(top[from])))) {
    return "reads rows and has no LIMIT.";
  }
  return null;
}

function aggregateSelectList(list: SqlToken[]): boolean {
  if (list[0]?.kind === "word" && ["ALL", "DISTINCT"].includes(list[0].text.toUpperCase())) list = list.slice(1);
  return list.length > 0 && splitExpressions(list, 0).every((expression) => {
    const alias = expression.findIndex((token) => token.depth === 0 && token.kind === "word" && token.text.toUpperCase() === "AS");
    const beforeAlias = expression.at(-2);
    if (alias !== -1) expression = expression.slice(0, alias);
    else if (["word", "identifier"].includes(expression.at(-1)?.kind ?? "") && beforeAlias?.kind === "symbol" && beforeAlias.text === ")") expression = expression.slice(0, -1);
    return totalExpression(expression);
  });
}

function splitExpressions(list: SqlToken[], depth: number): SqlToken[][] {
  const parts: SqlToken[][] = [[]];
  for (const token of list) {
    if (token.depth === depth && token.kind === "symbol" && token.text === ",") parts.push([]);
    else parts.at(-1)!.push(token);
  }
  return parts;
}

function totalExpression(list: SqlToken[]): boolean {
  const [name, open] = list;
  const close = list.at(-1);
  if (name?.kind !== "word" || open?.kind !== "symbol" || open.text !== "(" || close?.kind !== "symbol" || close.text !== ")" || close.depth !== name.depth) return false;
  if (list.findIndex((token, index) => index > 1 && token.kind === "symbol" && token.text === ")" && token.depth === name.depth) !== list.length - 1) return false;
  const args = splitExpressions(list.slice(2, -1), name.depth + 1);
  const word = name.text.toUpperCase();
  if (aggregates.has(word)) {
    return args.every((arg) => arg.length > 0) && (args.length === 1 || word === "GROUP_CONCAT" && args.length === 2);
  }
  if (!["COALESCE", "IFNULL"].includes(word) || args.length < 2 || word === "IFNULL" && args.length !== 2) return false;
  return args.some(totalExpression) && args.every((arg) => totalExpression(arg) || constantExpression(arg));
}

function constantExpression(list: SqlToken[]): boolean {
  if (list.length === 2 && list[0].kind === "symbol" && ["+", "-"].includes(list[0].text)) list = list.slice(1);
  const token = list[0];
  return list.length === 1 && (token.kind === "string" || token.kind === "word" && token.text.toUpperCase() === "NULL" || token.kind === "symbol" && /^\d+(?:\.\d+)?$/.test(token.text));
}

function indexOfWord(tokens: SqlToken[], word: string): number {
  return tokens.findIndex((token) => token.kind === "word" && token.text.toUpperCase() === word);
}
