import { refusal, type RefusalError } from "../kernel/refusal";
import { sqlStatementTokens, sqlTokens, type SqlToken } from "../lib/sql-tokens";

export const SCHEMA_TRANSFORMS_FILE = "schema-transforms.json";
const MAX_TRANSITIONS = 256;
const MAX_STATEMENTS = 100;
const MAX_STATEMENT_BYTES = 20_000;
const MAX_DOCUMENT_BYTES = 20 * 1024 * 1024;
const MAX_ROWS = 100_000;
const RESERVED = new Set([
  "sqlite_master", "sqlite_schema", "sqlite_temp_master", "sqlite_temp_schema",
  "__konae_schema_state", "__konae_schema_history",
]);
const DENIED_FUNCTIONS = new Set(["load_extension", "readfile", "writefile", "edit", "fts3_tokenizer", "http_get", "http_post", "http_request", "curl", "fetch"]);

type Transition = {
  id: string;
  predecessor?: string | null;
  before: string;
  after: string;
  maxRows: number;
  statements: string[];
};

/** Checks document shape, order, bounds and the SQL allowlist without simulating data or history. */
export function schemaTransformsRefusal(source: string): RefusalError | null {
  if (new TextEncoder().encode(source).length > MAX_DOCUMENT_BYTES) {
    return refusal("schema-transform.document-invalid", `schema-transforms.json must be at most ${MAX_DOCUMENT_BYTES} bytes.`);
  }
  let document: unknown;
  try { document = JSON.parse(source); }
  catch { return refusal("schema-transform.document-invalid", "schema-transforms.json must be valid JSON."); }
  if (!isRecord(document) || !exactKeys(document, ["version", "transitions"]) || !Number.isInteger(document.version) || (document.version as number) < 0 || (document.version as number) > 0xffff_ffff || !Array.isArray(document.transitions)) return invalidDocument();
  if (document.version !== 1) return refusal("schema-transform.document-invalid", "schema-transforms.json version must be 1.");
  if (document.transitions.length > MAX_TRANSITIONS) {
    return refusal("schema-transform.document-invalid", `schema-transforms.json may contain at most ${MAX_TRANSITIONS} transitions.`);
  }
  let previous: Transition | undefined;
  const ids = new Set<string>();
  for (const value of document.transitions) {
    if (!isRecord(value) || !exactKeys(value, ["id", "before", "after", "maxRows", "statements"], ["predecessor"])) return invalidDocument();
    if (typeof value.id !== "string" || !(value.predecessor === undefined || value.predecessor === null || typeof value.predecessor === "string") || typeof value.before !== "string" || typeof value.after !== "string" || !Number.isInteger(value.maxRows) || (value.maxRows as number) < 0 || !Array.isArray(value.statements) || value.statements.some((statement) => typeof statement !== "string")) return invalidDocument();
    const transition = value as unknown as Transition;
    if (typeof transition.id !== "string" || transition.id.length < 1 || transition.id.length > 100 || [...transition.id].some((character) => !/[a-z0-9._-]/.test(character))) {
      return refusal("schema-transform.document-invalid", "A transition ID must use lowercase letters, digits, dots, underscores or hyphens, up to 100 bytes.");
    }
    if (ids.has(transition.id)) return refusal("schema-transform.document-invalid", `Transition ${transition.id} appears more than once.`);
    ids.add(transition.id);
    const expectedPredecessor = previous?.id;
    if (expectedPredecessor === undefined ? transition.predecessor != null : transition.predecessor !== expectedPredecessor) {
      return refusal("schema-transform.document-invalid", `Transition ${transition.id} must name its immediately preceding transition as predecessor.`);
    }
    if (previous && previous.after !== transition.before) {
      return refusal("schema-transform.document-invalid", `Transition ${transition.id} does not start at the previous transition's after fingerprint.`);
    }
    if (!validFingerprint(transition.before) || !validFingerprint(transition.after)) {
      return refusal("schema-transform.document-invalid", `Transition ${transition.id} must use lowercase sha256 fingerprints.`);
    }
    if (!Number.isSafeInteger(transition.maxRows) || transition.maxRows < 1 || transition.maxRows > MAX_ROWS) {
      return refusal("schema-transform.document-invalid", `Transition ${transition.id} maxRows must be from 1 to ${MAX_ROWS}.`);
    }
    if (transition.statements.length < 1 || transition.statements.length > MAX_STATEMENTS) {
      return refusal("schema-transform.document-invalid", `Transition ${transition.id} must contain from 1 to ${MAX_STATEMENTS} statements.`);
    }
    for (const statement of transition.statements) {
      const byteLength = new TextEncoder().encode(statement).length;
      if (byteLength > MAX_STATEMENT_BYTES) return refused(`Transition ${transition.id} has a statement over ${MAX_STATEMENT_BYTES} bytes.`);
      const problem = validateStatement(statement);
      if (problem) return refused(`Transition ${transition.id}: ${problem}`);
    }
    previous = transition;
  }
  return null;
}

function validateStatement(statement: string): string | null {
  const tokens = sqlTokens(statement);
  const semicolons = tokens.filter((token) => token.kind === "symbol" && token.text === ";");
  if (semicolons.length > 1 || (semicolons.length === 1 && tokens.at(-1) !== semicolons[0])) {
    return "Write one supported SQL statement without a second statement.";
  }
  const scripts = sqlStatementTokens(statement);
  if (scripts.length !== 1) return "Write one supported SQL statement without a second statement.";
  const words = canonicalStatementTokens(statement, tokens);
  if (words.at(-1) === ";") words.pop();
  if (words.length === 0) return "Write one supported SQL statement without a second statement.";
  if (hasSequence(words, ["on", "conflict", "rollback"])) {
    return "ROLLBACK conflict policies are refused because they can end the caller's transaction.";
  }
  let result: { tableAt: number | null; error?: string };
  switch (words[0]) {
    case "create": result = validateCreate(words); break;
    case "alter": result = validateAlter(words); break;
    case "insert": result = validateInsert(words); break;
    case "update": result = validateUpdate(words); break;
    default: return "Use CREATE TABLE/INDEX, ALTER TABLE RENAME/ADD COLUMN, INSERT or UPDATE.";
  }
  if (result.error) return result.error;
  const tableAt = result.tableAt;
  if (tableAt !== null && isForbiddenObject(words[tableAt]!)) return "A transition cannot access SQLite schema tables or Konae's reserved metadata tables.";
  if (words.some(isForbiddenObject)) return "A transition cannot access SQLite schema tables or Konae's reserved metadata tables.";
  for (let index = 0; index < words.length - 1; index += 1) {
    if (words[index + 1] === "(" && DENIED_FUNCTIONS.has(words[index]!)) return "Filesystem, network and extension functions are refused in transitions.";
  }
  return null;
}

function validateCreate(words: string[]): { tableAt: number | null; error?: string } {
  let object: "table" | "index";
  let at: number;
  if (words[1] === "table") { object = "table"; at = 2; }
  else if (words[1] === "index") { object = "index"; at = 2; }
  else if (words[1] === "unique" && words[2] === "index") { object = "index"; at = 3; }
  else return { tableAt: null, error: "CREATE is limited to TABLE and INDEX." };
  at = optionalIfNotExists(words, at);
  const name = words[at];
  if (!name) return { tableAt: null, error: `CREATE ${object} needs a name.` };
  if (isQualified(name) || name === "temp" || name === "temporary") return { tableAt: null, error: "Use an unqualified app table or index name." };
  if (object === "table" && words[at + 1] === "as" && words[at + 2] === "select") {
    return { tableAt: null, error: "CREATE TABLE AS SELECT is refused. Use CREATE TABLE and a bounded INSERT SELECT." };
  }
  if (object === "index" && !words.includes("on")) return { tableAt: null, error: "CREATE INDEX must name its table with ON." };
  return { tableAt: at };
}

function validateAlter(words: string[]): { tableAt: number | null; error?: string } {
  if (words[1] !== "table" || words.length < 6) return { tableAt: null, error: "ALTER is limited to ALTER TABLE RENAME or ADD COLUMN." };
  if (isQualified(words[2] ?? "")) return { tableAt: null, error: "ALTER TABLE must name an unqualified app table." };
  const allowed = (words[3] === "rename" && words[4] === "to" && words.length === 6)
    || (words[3] === "rename" && words[4] === "column" && words[6] === "to" && words.length === 8)
    || (words[3] === "add" && words[4] === "column" && words.length >= 7);
  return allowed ? { tableAt: 2 } : { tableAt: null, error: "ALTER TABLE supports only RENAME TO, RENAME COLUMN, or ADD COLUMN." };
}

function validateInsert(words: string[]): { tableAt: number | null; error?: string } {
  if (words.includes("replace")) return { tableAt: null, error: "INSERT OR REPLACE and REPLACE are refused." };
  if (hasSequence(words, ["or", "rollback"])) return { tableAt: null, error: "ROLLBACK conflict policies are refused because they can end the caller's transaction." };
  const into = words.indexOf("into");
  if (into < 0) return { tableAt: null, error: "INSERT must use INTO." };
  if (!words[into + 1]) return { tableAt: null, error: "INSERT INTO needs an app table." };
  if (isQualified(words[into + 1]!)) return { tableAt: null, error: "INSERT INTO must name an unqualified app table." };
  return words.some((word) => ["values", "select", "default"].includes(word))
    ? { tableAt: into + 1 }
    : { tableAt: null, error: "INSERT must provide VALUES, SELECT, or DEFAULT VALUES." };
}

function validateUpdate(words: string[]): { tableAt: number | null; error?: string } {
  let at = 1;
  if (words[at] === "or") {
    if (words[at + 1] === "rollback") return { tableAt: null, error: "ROLLBACK conflict policies are refused because they can end the caller's transaction." };
    if (!["abort", "fail", "ignore"].includes(words[at + 1] ?? "")) return { tableAt: null, error: "UPDATE OR supports ABORT, FAIL or IGNORE." };
    at += 2;
  }
  if (!words[at]) return { tableAt: null, error: "UPDATE needs an app table." };
  if (isQualified(words[at]!) || !words.slice(at + 1).includes("set")) return { tableAt: null, error: "UPDATE must name an unqualified app table and SET columns." };
  return { tableAt: at };
}

function canonicalStatementTokens(statement: string, tokens: SqlToken[]): string[] {
  const words: string[] = [];
  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index]!;
    if (token.kind === "string") {
      const raw = statement.slice(token.start + 1, token.end - 1).replaceAll("''", "'");
      words.push(`'${raw}'`);
    } else if (token.kind === "word") {
      let value = token.text.toLowerCase();
      let end = token.end;
      while (true) {
        const separator = tokens[index + 1];
        if (separator?.kind !== "symbol" || separator.text !== "." || separator.start !== end) break;
        value += ".";
        end = separator.end;
        index += 1;
        const next = tokens[index + 1];
        if (next?.kind !== "word" || next.start !== end) break;
        value += next.text.toLowerCase();
        end = next.end;
        index += 1;
      }
      words.push(value);
    } else {
      words.push(token.text.toLowerCase());
    }
  }
  return words;
}

function optionalIfNotExists(words: string[], at: number): number {
  return words[at] === "if" && words[at + 1] === "not" && words[at + 2] === "exists" ? at + 3 : at;
}

function hasSequence(words: string[], sequence: string[]): boolean {
  return words.some((_, index) => sequence.every((word, offset) => words[index + offset] === word));
}

function isQualified(name: string): boolean { return name.includes("."); }
function isForbiddenObject(name: string): boolean { return RESERVED.has(name.toLowerCase().split(".").at(-1) ?? name.toLowerCase()); }
function validFingerprint(value: unknown): value is string { return typeof value === "string" && value.length === 71 && /^sha256:[0-9a-f]{64}$/.test(value); }
function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value); }
function exactKeys(value: Record<string, unknown>, required: string[], optional: string[] = []): boolean {
  const keys = Object.keys(value);
  return required.every((key) => keys.includes(key)) && keys.every((key) => required.includes(key) || optional.includes(key));
}
function invalidDocument(): RefusalError { return refusal("schema-transform.document-invalid", "schema-transforms.json must be valid JSON."); }
function refused(message: string): RefusalError { return refusal("schema-transform.statement-refused", message); }
