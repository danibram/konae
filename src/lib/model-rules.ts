// The refusals on the way from a Database Model to a published database: a
// release names its accepted model, the database matches that model, and a
// migration plan changes something. kernel-rs/src/model_rules.rs gives the same
// sentences, and kernel-rs/src/database-model-cases.json holds the cases both
// test suites run.
import type { Database } from "sql.js";
import { refusal, type RefusalError } from "../kernel/refusal";

/** A release publishes the model the database was migrated to: the edited source is the accepted one, with a revision. */
export function acceptedModel(model: { source: string; acceptedSource: string; revision: string }): RefusalError | null {
  if (model.source === model.acceptedSource && model.revision !== "") return null;
  return refusal("schema.model-not-accepted", "Publish the accepted database model. Apply its migration before publishing.");
}

/** `matches` is whether the schema the accepted model derives and the database's schema compare equal under `schemaTokens`. */
export function deployedModel(matches: boolean): RefusalError | null {
  if (matches) return null;
  return refusal("schema.model-differs-from-database", "The accepted database model does not match the deployed schema. Apply its migration before publishing.");
}

/** A plan with no statements has nothing to apply. */
export function plannedStatements(statements: readonly string[]): RefusalError | null {
  if (statements.length > 0) return null;
  return refusal("schema.no-changes", "The Drizzle model has no schema changes to migrate.");
}

export type SchemaTokens = Array<[kind: string, name: string, tokens: string[]]>;

/**
 * The schema of `db` as kernel-rs/src/mcp.rs `schema_tokens` compares it:
 * defaults, constraints, indexes, triggers and views stay; CHECKs, which a
 * Database Model cannot state, leave; column definitions sort by name.
 */
export function schemaTokens(db: Database): SchemaTokens {
  const rows = db.exec("SELECT type, name, sql FROM sqlite_master WHERE name NOT GLOB 'sqlite_*' ORDER BY type, name")[0]?.values ?? [];
  return rows.map(([kind, name, sql]) => {
    const tokens = tokenize(typeof sql === "string" ? sql : "");
    for (let at = 0; at + 2 < tokens.length; at++) {
      if (tokens[at] === "autoincrement" && tokens[at + 1] === "not" && tokens[at + 2] === "null") tokens.splice(at + 1, 2);
    }
    for (let at = 0; at + 1 < tokens.length; at++) {
      if (tokens[at] !== "default") continue;
      if (tokens[at + 1] === "false") tokens[at + 1] = "0";
      if (tokens[at + 1] === "true") tokens[at + 1] = "1";
      if (at + 3 < tokens.length && tokens[at + 2] === "not" && tokens[at + 3] === "null") {
        tokens.splice(at, 4, "not", "null", tokens[at], tokens[at + 1]);
        at += 2;
      }
    }
    const kept = kind === "table" ? canonicalColumnOrder(dropChecks(tokens)) : tokens;
    return [String(kind), String(name), kept];
  });
}

const quotedWords = ["null", "true", "false", "current_date", "current_time", "current_timestamp"];

function tokenize(sql: string): string[] {
  const chars = [...sql];
  const tokens: string[] = [];
  let at = 0;
  while (at < chars.length) {
    const c = chars[at++];
    if (/\s/u.test(c) || c === ";") continue;
    let token = "";
    if (c === "'" || c === '"' || c === "`" || c === "[") {
      const end = c === "[" ? "]" : c;
      if (c === "'") token += c;
      while (at < chars.length) {
        const next = chars[at++];
        if (next !== end) { token += next; continue; }
        if (chars[at] === end) { token += next; at++; } else break;
      }
      if (c === "'") token += c;
      else {
        token = asciiLower(token);
        if (quotedWords.includes(token)) token = `identifier:${token}`;
      }
    } else if (isWordChar(c)) {
      token += c;
      while (at < chars.length && isWordChar(chars[at])) token += chars[at++];
      token = asciiLower(token);
    } else token = c;
    tokens.push(token);
  }
  return tokens;
}

function isWordChar(c: string): boolean {
  return c === "_" || /[\p{L}\p{N}]/u.test(c);
}

function asciiLower(text: string): string {
  return text.replace(/[A-Z]/g, (letter) => letter.toLowerCase());
}

/** Leaves out each CHECK group and a CONSTRAINT name before it. */
function dropChecks(tokens: string[]): string[] {
  const kept: string[] = [];
  let at = 0;
  while (at < tokens.length) {
    const named = tokens[at] === "constraint" && tokens[at + 2] === "check";
    const start = named ? at + 2 : at;
    if (tokens[start] === "check" && tokens[start + 1] === "(") {
      let depth = 0;
      at = start + 1;
      while (at < tokens.length) {
        if (tokens[at] === "(") depth++;
        else if (tokens[at] === ")") depth--;
        at++;
        if (depth === 0) break;
      }
      continue;
    }
    kept.push(tokens[at]);
    at++;
  }
  return kept;
}

const constraintWords = ["constraint", "primary", "unique", "check", "foreign"];

/** ALTER TABLE appends columns even when the model declares them earlier. Columns sort by name; composite constraints keep their order. */
function canonicalColumnOrder(tokens: string[]): string[] {
  if (tokens[0] !== "create" || tokens[1] !== "table") return tokens;
  const open = tokens.indexOf("(");
  if (open === -1) return tokens;
  let depth = 1;
  let start = open + 1;
  const columns: string[][] = [];
  const constraints: string[][] = [];
  let close = -1;
  for (let at = open + 1; at < tokens.length; at++) {
    if (tokens[at] === "(") depth++;
    else if (tokens[at] === ")") depth--;
    if (depth === 0 || (depth === 1 && tokens[at] === ",")) {
      const definition = tokens.slice(start, at);
      if (definition.length > 0) (constraintWords.includes(definition[0]) ? constraints : columns).push(definition);
      start = at + 1;
    }
    if (depth === 0) { close = at; break; }
  }
  if (close === -1) return tokens;
  columns.sort(compareDefinitions);
  const normalized = tokens.slice(0, open + 1);
  for (const definition of [...columns, ...constraints]) {
    if (normalized.length > open + 1) normalized.push(",");
    normalized.push(...definition);
  }
  return [...normalized, ...tokens.slice(close)];
}

/** Rust's `Vec<String>` order: element by element, by code point, then by length. */
function compareDefinitions(left: string[], right: string[]): number {
  for (let at = 0; at < Math.min(left.length, right.length); at++) {
    if (left[at] !== right[at]) return left[at] < right[at] ? -1 : 1;
  }
  return left.length - right.length;
}
