/**
 * A single-pass SQL tokenizer for the places that need to read a statement
 * without a parser: the Validation Gate and the SQL console. Comments, string
 * literals and quoted identifiers are consumed whole, so a keyword or a `;`
 * inside one never counts.
 */
export type SqlToken =
  /** A bare word as written, e.g. `delete`, `tasks`. */
  | { kind: "word"; text: string; depth: number }
  /** A quoted identifier ("x", `x` or [x]), without its quotes. */
  | { kind: "identifier"; text: string; depth: number }
  /** A string literal. Its contents are never read. */
  | { kind: "string"; depth: number }
  /** Any other character that is not whitespace: `(`, `=`, `,`, `;` and so on. */
  | { kind: "symbol"; text: string; depth: number };

export function sqlTokens(sql: string): SqlToken[] {
  const tokens: SqlToken[] = [];
  let depth = 0;
  let index = 0;
  while (index < sql.length) {
    const char = sql[index];
    const next = sql[index + 1];
    if (char === "-" && next === "-") {
      const end = sql.indexOf("\n", index + 2);
      index = end === -1 ? sql.length : end + 1;
    } else if (char === "/" && next === "*") {
      const end = sql.indexOf("*/", index + 2);
      index = end === -1 ? sql.length : end + 2;
    } else if (char === "'" || char === '"' || char === "`" || char === "[") {
      const close = char === "[" ? "]" : char;
      let end = index + 1;
      let text = "";
      while (end < sql.length) {
        if (sql[end] === close) {
          if (close !== "]" && sql[end + 1] === close) {
            text += close;
            end += 2;
            continue;
          }
          break;
        }
        text += sql[end];
        end += 1;
      }
      tokens.push(char === "'" ? { kind: "string", depth } : { kind: "identifier", text, depth });
      index = end + 1;
    } else if (/[A-Za-z_\u0080-\uffff]/.test(char)) {
      // SQLite reads every character at or above U+0080 as part of an
      // identifier, so `éwhere` or `t\u00a0WHERE` is one name, never a WHERE.
      // This branch comes before the whitespace one for that reason.
      const match = /^[A-Za-z_\u0080-\uffff][A-Za-z0-9_$\u0080-\uffff]*/.exec(sql.slice(index))![0];
      tokens.push({ kind: "word", text: match, depth });
      index += match.length;
    } else if (/[0-9]/.test(char)) {
      // A number, including forms like 1e5 or 0x1F, so its letters never read as a word.
      const match = /^[0-9][0-9A-Za-z_.]*/.exec(sql.slice(index))![0];
      tokens.push({ kind: "symbol", text: match, depth });
      index += match.length;
    } else if (/\s/.test(char)) {
      index += 1;
    } else {
      if (char === ")") depth = Math.max(0, depth - 1);
      tokens.push({ kind: "symbol", text: char, depth });
      if (char === "(") depth += 1;
      index += 1;
    }
  }
  return tokens;
}

/** The bare words among `tokens`, upper-cased. Strings and quoted identifiers are not words. */
export function sqlWordsOf(tokens: SqlToken[]): string[] {
  return tokens.flatMap((token) => token.kind === "word" ? [token.text.toUpperCase()] : []);
}

/**
 * The tokens of each statement in a script, split on `;`. A trigger body
 * (`CREATE TRIGGER ... BEGIN ...; END`) stays one statement. Empty statements
 * are dropped.
 */
export function sqlStatementTokens(sql: string): SqlToken[][] {
  const statements: SqlToken[][] = [];
  let current: SqlToken[] = [];
  let triggerBody = false;
  // CASE ... END can sit inside a trigger body; its END does not close the body.
  let openCases = 0;
  for (const token of sqlTokens(sql)) {
    if (token.kind === "word") {
      const word = token.text.toUpperCase();
      if (word === "BEGIN" && !triggerBody && isTrigger(current)) triggerBody = true;
      else if (word === "CASE" && triggerBody) openCases += 1;
      else if (word === "END" && triggerBody) {
        if (openCases > 0) openCases -= 1;
        else triggerBody = false;
      }
    }
    if (token.kind === "symbol" && token.text === ";" && !triggerBody) {
      if (current.length > 0) statements.push(current);
      current = [];
      continue;
    }
    current.push(token);
  }
  if (current.length > 0) statements.push(current);
  return statements;
}

function isTrigger(tokens: SqlToken[]): boolean {
  const words = sqlWordsOf(tokens);
  return words[0] === "CREATE" && words.slice(1, 4).includes("TRIGGER");
}
