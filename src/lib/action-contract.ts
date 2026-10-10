/**
 * The Action Contract: a machine-readable description of every Server Action
 * a Capsule declares, written from `actions.json` and `schema.sql`. It states
 * what a caller sends, what it gets back, who may call and what the Kernel
 * refuses, so a second backend can be held to the same answers by the
 * conformance runner in src/lib/action-conformance.ts.
 *
 * The generator reads SQL with the tokenizer the Validation Gate uses. It does
 * not run SQLite, so it works in the editor, in the CLI and in the export kit.
 * Output column names follow SQLite's rule for a result column: the alias when
 * there is one, otherwise the column's own name. An expression with no alias
 * gets the text SQLite happens to print for it, which another backend would
 * not reproduce, so the contract lists it as unnamed and the Gate warns.
 *
 * Error messages are not written here. They come from the SDK contract in
 * src/sdk/contract.ts, whose test checks each against the Kernel's source.
 */
import { parseServerActionSource, type HttpBinding, type RouteCaller, type ScheduleDeclaration, type ServerActionDefinition, type ServerActionProgram } from "../kernel/actions";
import { sdkContract, type SdkError } from "../sdk/contract";
import { limitIndex } from "./query-bound";
import { sqlStatementTokens, sqlTokens, type SqlToken } from "./sql-tokens";
import contractSchema from "./action-contract.schema.json";

export const actionContractSchema = contractSchema as unknown as JsonSchema;
export const ACTION_CONTRACT_FILE = "action-contract.json";
export const ACTION_CONTRACT_MARKDOWN = "action-contract.md";
export const ACTION_CONTRACT_SCHEMA_FILE = "action-contract.schema.json";

/** SQLite type affinity, plus "any" for a column declared without a type and "unknown" for an expression. */
export type ColumnType = "integer" | "real" | "text" | "blob" | "numeric" | "any" | "unknown";

export type ContractColumn = {
  name: string;
  type: ColumnType;
  /** null when the generator cannot tell. */
  nullable: boolean | null;
  /** The table column it reads, when it reads one directly. */
  from?: { table: string; column: string };
};

export type ContractOutput =
  | {
    kind: "rows";
    columns: ContractColumn[];
    /** Result columns with no name a second backend could match: expressions without AS. */
    unnamed: { expression: string }[];
  }
  | { kind: "changes"; fields: ["changes", "lastInsertId"] }
  /** Reserved for actions that record a Signal (ADR-0010). The generator does not write it yet. */
  | { kind: "signal"; signal: string };

export type ContractInput = {
  key: string;
  type: "string" | "number" | "boolean";
  /** A required input must be present; a nullable one may be omitted or null. */
  required: boolean;
  nullable: boolean;
  /** UTF-16 code units, for string inputs. */
  maxLength?: number;
};

export type ContractBinding =
  | { source: "input"; key: string }
  | { source: "actor"; field: "id" | "role" }
  | { source: "kernel"; field: "now" }
  | { source: "literal"; value: string | number | null };

export type ContractCaller = {
  kind: "visitor" | "person" | "service";
  proof: string;
  actor: "none" | "user id" | "key id";
};

export type ContractHttp = {
  method: "GET" | "POST";
  path: string;
  input: string;
  signature?: { header: string; secret: string; scheme: string; windowSeconds: number; acceptedOnce: true };
  callers: ContractCaller[];
  /** The `callers` list actions.json declares, when it declares one. */
  declaredCallers?: RouteCaller[];
  /**
   * When the route is the action's only door, so the runtime endpoint refuses
   * the action: always for a signed route or one with declared callers, and on
   * a private Deployment for an unsigned one (ADR-0011, update of 2026-11-06).
   */
  onlyDoor: "always" | "on a private Deployment";
};

export type ContractError = {
  message: string;
  when: string;
  /** Where the refusal is seen: the runtime endpoint, the HTTP route, or both. */
  via: "runtime" | "http" | "both";
  /** The HTTP status, for refusals on a route. */
  status?: number;
};

export type ContractLimits = {
  /** The top-level LIMIT of a query, or null when the bound is a single row. */
  limit: number | null;
  bound: "limit" | "single-row" | "none";
  maxRows: number;
  maxResultBytes: number;
  statementSeconds: number;
  rate: { caller: string; calls: number; seconds: number }[];
  maxRequestBytes?: number;
};

export type ContractAction = {
  name: string;
  kind: "query" | "mutation" | "signal";
  access: "anonymous" | "authenticated";
  /** Whether a binding passes the caller's ID; such an action refuses a caller without one. */
  actorBound: boolean;
  inputs: ContractInput[];
  output: ContractOutput;
  /**
   * The Signal the action records when it succeeds, and for a mutation only
   * when it changed a row, with the input keys copied into its properties and
   * `actor` for the caller's ID (ADR-0010).
   */
  signal?: { name: string; properties: string[] };
  /** A mutation's refusal when its statement changes no row; it is listed among the errors too. */
  guard?: string;
  /**
   * When the Kernel's timer also runs the mutation, with no caller: `actor`
   * binds null, the guard does not apply, and Usage counts the run under
   * `scheduled` (ADR-0015).
   */
  schedule?: ScheduleDeclaration;
  runtime: { endpoint: string; tool: string; sdk: string; callers: ContractCaller[] };
  http?: ContractHttp;
  errors: ContractError[];
  limits: ContractLimits;
  statement: string;
  bindings: ContractBinding[];
};

export type ActionContract = {
  format: "littlebox-action-contract";
  version: 1;
  project?: { id: string; name: string };
  /** Refusals any action can return, and the one for a name no action declares. */
  errors: ContractError[];
  actions: ContractAction[];
  /** Plain sentences on what a reader must not assume. */
  notes: string[];
};

export type ContractWarning = { action: string; message: string };

// The numbers below are the Kernel's. src/lib/action-contract.test.ts reads
// kernel-rs/src/limits.rs and main.rs and fails when they move.
const RATE = [
  { caller: "visitor, per client address and Deployment", calls: 30, seconds: 10 },
  { caller: "signed-in person or API key, each", calls: 120, seconds: 10 },
];
const MAX_ROWS = 1000;
const MAX_RESULT_BYTES = 2 * 1024 * 1024;
const STATEMENT_SECONDS = 2;
const MAX_ROUTE_BODY = 64 * 1024;
const SIGNATURE_WINDOW = 300;

const RUNTIME = { endpoint: "/_littlebox/mcp", tool: "actions_invoke", sdk: "miniapp.actions.call" };
const SIGNATURE_SCHEME = "t=<unix seconds>,n=<nonce>,v1=<hex HMAC-SHA256, keyed with the secret, of t, n, the method, the path, the canonical query string and the raw body, joined by newlines>";

/**
 * Writes the contract for a Capsule's actions.json and schema.sql. Throws the
 * parser's own message when actions.json does not parse: a Capsule the Gate
 * refuses has no contract.
 */
export function generateActionContract(input: {
  actionsSource: string;
  schemaSource: string;
  project?: { id: string; name: string };
}): { contract: ActionContract; warnings: ContractWarning[] } {
  const program = parseServerActionSource(input.actionsSource);
  return contractFromProgram(program, input.schemaSource, input.project);
}

export function contractFromProgram(program: ServerActionProgram, schemaSource: string, project?: { id: string; name: string }) {
  const tables = parseSchemaTables(schemaSource);
  const warnings: ContractWarning[] = [];
  const actions = Object.keys(program.actions).sort().map((name) => {
    const action = contractAction(program.actions[name], tables);
    if (action.output.kind === "rows") {
      for (const column of action.output.unnamed) {
        warnings.push({ action: name, message: unnamedWarning(name, column.expression) });
      }
    }
    return action;
  });
  const contract: ActionContract = {
    format: "littlebox-action-contract",
    version: 1,
    ...(project ? { project: { id: project.id, name: project.name } } : {}),
    errors: [
      fromSdk("Server action not declared.", "runtime"),
      { ...fromSdk("Sign in to use Littlebox MCP.", "runtime"), when: "the Deployment is private and the caller has no session, whatever the action's name" },
      fromSdk("Arguments must be an object.", "runtime"),
      fromSdk("No route accepts POST on this path.", "http"),
    ],
    actions,
    notes: [
      "Generated from actions.json and schema.sql. Do not edit it; change those files and generate it again.",
      "A query answers an array of row objects keyed by the column names listed. A mutation answers { changes, lastInsertId }.",
      "Column types are SQLite affinities read from schema.sql. SQLite does not enforce them, so a value can still arrive as another JSON type when it was stored as one.",
      "Nullability follows NOT NULL and INTEGER PRIMARY KEY in schema.sql. A column read through the right side of a LEFT JOIN is nullable.",
      "A SQLite error, such as a UNIQUE constraint, is returned with SQLite's own text.",
      "On a private Deployment every runtime call needs a session, including anonymous actions. An HTTP route answers the callers it takes: without a callers list, a signed route takes the signature or an API key, and an unsigned one anyone on a public Deployment and an API key or a session on a private one. An action whose route does not answer anyone answers only there.",
    ],
  };
  return { contract, warnings };
}

/** The Gate's warning for a result column a second backend cannot name. */
export function unnamedWarning(action: string, expression: string): string {
  return `Server action ${action} returns ${expression} without a name. Add AS <name> so every backend returns the same key.`;
}

function contractAction(action: ServerActionDefinition, tables: SchemaTables): ContractAction {
  // The parser refuses "granted" until grants exist, so it never reaches here.
  if (action.access === "granted") throw new Error(`Action ${action.name} uses "access": "granted", which the Kernel does not support yet.`);
  const access = action.access;
  const inputs: ContractInput[] = [];
  const bindings: ContractBinding[] = action.bindings.map((binding) => {
    if (binding.source !== "input") return binding;
    if (!inputs.some((input) => input.key === binding.key)) {
      const nullable = binding.nullable === true;
      inputs.push({
        key: binding.key,
        type: binding.type,
        required: !nullable,
        nullable,
        ...(binding.type === "string" ? { maxLength: binding.maxLength ?? 10_000 } : {}),
      });
    }
    return { source: "input", key: binding.key };
  });
  // The role binds null for a caller without one, so only the ID refuses them.
  const actorBound = action.bindings.some((binding) => binding.source === "actor" && binding.field === "id");
  const output: ContractOutput = action.kind === "query"
    ? { kind: "rows", ...deriveOutputColumns(action.statement, tables) }
    : action.kind === "signal"
      ? { kind: "signal", signal: action.signal?.name ?? action.name }
      : { kind: "changes", fields: ["changes", "lastInsertId"] };
  const http = action.http ? contractHttp(action.http, action.kind) : undefined;
  return {
    name: action.name,
    kind: action.kind,
    access,
    actorBound,
    inputs,
    output,
    ...(action.signal ? { signal: { name: action.signal.name, properties: [...action.signal.properties] } } : {}),
    ...(action.guard ? { guard: action.guard } : {}),
    ...(action.schedule ? { schedule: { ...action.schedule } } : {}),
    runtime: { ...RUNTIME, callers: http?.onlyDoor === "always" ? [] : runtimeCallers(access, Boolean(http)) },
    ...(http ? { http } : {}),
    errors: actionErrors(action, inputs, actorBound),
    limits: actionLimits(action),
    statement: action.statement,
    bindings,
  };
}

function runtimeCallers(access: "anonymous" | "authenticated", routed: boolean): ContractCaller[] {
  const session = "the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>";
  const person: ContractCaller = { kind: "person", proof: routed ? `${session}; a public Deployment only, since on a private one the action answers only on its route` : session, actor: "user id" };
  return access === "anonymous"
    ? [{ kind: "visitor", proof: "none; a public Deployment only", actor: "none" }, person]
    : [person];
}

function contractHttp(http: HttpBinding, kind: "query" | "mutation" | "signal"): ContractHttp {
  const declared = http.callers;
  const takes = (caller: RouteCaller) => declared ? declared.includes(caller) : caller !== "session" || !http.signature;
  const callers: ContractCaller[] = [];
  if (http.signature) callers.push({ kind: "visitor", proof: `the ${http.signature.header} signature`, actor: "none" });
  else if (!declared) callers.push({ kind: "visitor", proof: "none; a public Deployment only", actor: "none" });
  if (takes("key")) callers.push({ kind: "service", proof: `Authorization: Bearer <API key that names this action>${http.signature ? "; it replaces the signature" : ""}`, actor: "key id" });
  if (takes("session")) callers.push({ kind: "person", proof: "the app session cookie, from the app's own origin", actor: "user id" });
  return {
    method: http.method,
    path: http.path,
    input: kind === "query"
      ? "the query string, each value typed by its input's declared type"
      : `a JSON object body of at most ${MAX_ROUTE_BODY / 1024} KiB`,
    ...(http.signature ? {
      signature: { header: http.signature.header, secret: http.signature.secret, scheme: SIGNATURE_SCHEME, windowSeconds: SIGNATURE_WINDOW, acceptedOnce: true as const },
    } : {}),
    callers,
    ...(declared ? { declaredCallers: [...declared] } : {}),
    onlyDoor: http.signature || declared ? "always" : "on a private Deployment",
  };
}

// --- Errors -------------------------------------------------------------------

const actionsCall = sdkContract.modules.find((module) => module.name === "actions")!.calls.find((call) => call.name === "call")!;
const httpDeclaration = sdkContract.modules.find((module) => module.name === "http")!.declarations.find((declaration) => declaration.key === "http")!;
const sdkErrors: readonly SdkError[] = [...sdkContract.errors, ...actionsCall.errors, ...(httpDeclaration.errors ?? [])];

/** A refusal the http module lists with its status, for a message the runtime also uses without one. */
function httpError(message: string): ContractError {
  const errors: readonly SdkError[] = httpDeclaration.errors ?? [];
  const error = errors.find((candidate) => candidate.message === message && candidate.status !== undefined);
  if (!error) throw new Error(`The http module has no route error "${message}".`);
  return { message: error.message, when: error.when, via: "http", status: error.status };
}

function fromSdk(message: string, via: ContractError["via"], values: Record<string, string> = {}): ContractError {
  const error = sdkErrors.find((candidate) => candidate.message === message);
  if (!error) throw new Error(`The SDK contract has no error "${message}".`);
  const fill = (text: string) => text.replace(/<(key|name)>/g, (whole, slot: string) => values[slot] ?? whole);
  return {
    message: fill(error.message),
    when: error.when,
    via,
    ...(via !== "runtime" && error.status !== undefined ? { status: error.status } : {}),
  };
}

function actionErrors(action: ServerActionDefinition, inputs: ContractInput[], actorBound: boolean): ContractError[] {
  const http = action.http;
  const onlyDoor = http ? Boolean(http.signature || http.callers) : false;
  // A binding refusal is seen wherever the action runs: on its route alone when that is its only door.
  const both = !http ? "runtime" : onlyDoor ? "http" : "both";
  const errors: ContractError[] = [];
  if (http) {
    errors.push({
      ...fromSdk("Action <name> answers only on its HTTP route, <key>.", "runtime", { name: action.name, key: `${http.method} ${http.path}` }),
      when: onlyDoor ? "always: the route declares a signature or callers" : "the Deployment is private and the route has no signature",
    });
  }
  if (action.access === "authenticated") errors.push(fromSdk("authentication-required", "runtime"));
  errors.push({ ...fromSdk("Sign in to use Littlebox MCP.", "runtime"), when: "the Deployment is private and the caller has no session" });
  if (actorBound && action.access === "anonymous") errors.push(fromSdk("This action requires an authenticated actor.", both));
  errors.push(fromSdk("Unexpected action input: <key>", both));
  for (const input of inputs) {
    if (input.required) errors.push(fromSdk("Missing action input: <key>", both, { key: input.key }));
    if (input.type === "string") {
      errors.push(fromSdk("<key> must be a non-empty string.", both, { key: input.key }));
      errors.push(fromSdk("<key> is too long.", both, { key: input.key }));
    } else if (input.type === "number") {
      errors.push(fromSdk("<key> must be a number.", both, { key: input.key }));
    } else {
      errors.push(fromSdk("<key> must be a boolean.", both, { key: input.key }));
    }
  }
  if (action.kind === "query") errors.push(fromSdk("Query result exceeds 2 MiB or 1000 rows.", both));
  // The app's own refusal: 403 on a route, set below with the route's statuses.
  if (action.guard) errors.push({ message: action.guard, when: "the statement changes no row, so the action's rule refused the call. Usage counts it under refused and no Signal is recorded", via: both });
  errors.push(fromSdk("rate-limited", "runtime"));
  if (http) {
    // A binding refusal answers 400 on a route, and a guard 403.
    for (const error of errors) if (error.via === "both" || error.via === "http") error.status = error.message === action.guard ? 403 : 400;
    const takes = (caller: RouteCaller) => http.callers ? http.callers.includes(caller) : caller !== "session" || !http.signature;
    if (http.signature) {
      errors.push(fromSdk("invalid-signature", "http"));
      errors.push(fromSdk("stale-timestamp", "http"));
      errors.push(fromSdk("replayed-signature", "http"));
    } else {
      errors.push({
        ...httpError("authentication-required"),
        when: http.callers ? "the request carries neither an API key nor a session the route takes" : "the Deployment is private and the request carries neither an API key nor a session",
      });
    }
    errors.push(fromSdk("invalid-api-key", "http"));
    if (takes("key")) errors.push(fromSdk("This API key may not call <name>.", "http", { name: action.name }));
    else errors.push(fromSdk("This route does not accept API keys.", "http"));
    if (http.method === "POST") {
      errors.push(fromSdk("The request body exceeds 64 KiB.", "http"));
      errors.push(fromSdk("The request body must be a JSON object.", "http"));
    } else {
      errors.push(fromSdk("Repeated query parameter: <key>", "http"));
      errors.push({
        ...fromSdk("A route does not answer HEAD. Send GET.", "http"),
        when: "a HEAD reaches the route; it is refused before any SQL, with Allow: GET. HTTP carries no body on a HEAD answer, so the caller sees the 405 and the header, not the message",
      });
    }
    errors.push({ ...fromSdk("rate-limited", "http"), status: 429 });
  }
  return errors;
}

function actionLimits(action: ServerActionDefinition): ContractLimits {
  // The parser has already refused a query that reads rows without a top-level
  // LIMIT, so a query without one returns a single row (ADR-0009).
  const query = action.kind === "query";
  const bound: ContractLimits["bound"] = !query ? "none" : hasTopLevelLimit(action.statement) ? "limit" : "single-row";
  return {
    limit: bound === "limit" ? topLevelLimit(action.statement) : null,
    bound,
    maxRows: MAX_ROWS,
    maxResultBytes: MAX_RESULT_BYTES,
    statementSeconds: STATEMENT_SECONDS,
    rate: RATE,
    ...(action.http?.method === "POST" ? { maxRequestBytes: MAX_ROUTE_BODY } : {}),
  };
}

function hasTopLevelLimit(sql: string): boolean {
  return limitIndex(sqlTokens(sql).filter((token) => token.depth === 0)) !== -1;
}

/** The number after a top-level LIMIT, or null when it is a parameter or an expression. */
function topLevelLimit(sql: string): number | null {
  const tokens = sqlTokens(sql).filter((token) => token.depth === 0);
  const index = limitIndex(tokens);
  if (index === -1) return null;
  const next = tokens[index + 1];
  const after = tokens[index + 2];
  if (next?.kind !== "symbol" || !/^\d+$/.test(next.text)) return null;
  if (after && !(after.kind === "word" && after.text.toUpperCase() === "OFFSET") && !(after.kind === "symbol" && after.text === ";")) return null;
  return Number(next.text);
}

// --- Schema -------------------------------------------------------------------

export type SchemaColumn = { name: string; type: ColumnType; nullable: boolean };
export type SchemaTables = Map<string, SchemaColumn[]>;

/** Tables and columns of every CREATE TABLE in schema.sql, keyed by lower-case table name. */
export function parseSchemaTables(schemaSql: string): SchemaTables {
  const tables: SchemaTables = new Map();
  for (const statement of sqlStatementTokens(schemaSql)) {
    const words = statement.map((token) => token.kind === "word" ? token.text.toUpperCase() : "");
    if (words[0] !== "CREATE") continue;
    let at = 1;
    if (words[at] === "TEMP" || words[at] === "TEMPORARY") at += 1;
    if (words[at] !== "TABLE") continue;
    at += 1;
    if (words[at] === "IF" && words[at + 1] === "NOT" && words[at + 2] === "EXISTS") at += 3;
    let name = nameOf(statement[at]);
    if (isSymbol(statement[at + 1], ".")) { at += 2; name = nameOf(statement[at]); }
    const open = statement[at + 1];
    if (!name || !isSymbol(open, "(")) continue;
    const columns: SchemaColumn[] = [];
    for (const definition of splitTopLevel(statement.slice(at + 2), 1)) {
      const first = definition[0];
      if (!first) continue;
      const head = first.kind === "word" ? first.text.toUpperCase() : "";
      if (["PRIMARY", "UNIQUE", "CHECK", "FOREIGN", "CONSTRAINT"].includes(head)) continue;
      const column = nameOf(first);
      if (column) columns.push(schemaColumn(column, definition.slice(1)));
    }
    tables.set(name.toLowerCase(), columns);
  }
  return tables;
}

function schemaColumn(name: string, rest: SqlToken[]): SchemaColumn {
  const typeWords: string[] = [];
  for (const token of rest) {
    if (token.kind !== "word") break;
    const word = token.text.toUpperCase();
    if (["CONSTRAINT", "PRIMARY", "NOT", "NULL", "UNIQUE", "CHECK", "DEFAULT", "COLLATE", "REFERENCES", "GENERATED", "AS"].includes(word)) break;
    typeWords.push(word);
  }
  const declared = typeWords.join(" ");
  const words = rest.map((token) => token.kind === "word" ? token.text.toUpperCase() : "");
  const notNull = words.some((word, index) => word === "NOT" && words[index + 1] === "NULL");
  // Only INTEGER PRIMARY KEY is the rowid; SQLite lets another primary key column hold NULL.
  const rowid = declared === "INTEGER" && words.includes("PRIMARY");
  return { name, type: affinity(declared), nullable: !(notNull || rowid) };
}

/** SQLite's affinity rules, section 3.1 of its datatype documentation. */
function affinity(declared: string): ColumnType {
  if (!declared) return "any";
  if (declared.includes("INT")) return "integer";
  if (declared.includes("CHAR") || declared.includes("CLOB") || declared.includes("TEXT")) return "text";
  if (declared.includes("BLOB")) return "blob";
  if (declared.includes("REAL") || declared.includes("FLOA") || declared.includes("DOUB")) return "real";
  return "numeric";
}

// --- Output columns -----------------------------------------------------------

/**
 * The result columns of a SELECT, named as SQLite names them, typed from the
 * schema where a column is read directly. Expressions with no alias are
 * returned separately as `unnamed`.
 */
export function deriveOutputColumns(statement: string, tables: SchemaTables): { columns: ContractColumn[]; unnamed: { expression: string }[] } {
  const tokens = sqlTokens(statement);
  // The first top-level SELECT names the columns, also for a compound select.
  const select = tokens.findIndex((token) => token.depth === 0 && isWord(token, "SELECT"));
  if (select === -1) return { columns: [], unnamed: [{ expression: statement.trim() }] };
  const end = tokens.findIndex((token, index) => index > select && token.depth === 0
    && token.kind === "word" && ["FROM", "WHERE", "GROUP", "HAVING", "ORDER", "LIMIT", "UNION", "EXCEPT", "INTERSECT", "WINDOW"].includes(token.text.toUpperCase()));
  const listEnd = end === -1 ? tokens.length : end;
  let list = tokens.slice(select + 1, listEnd);
  if (list[0] && (isWord(list[0], "DISTINCT") || isWord(list[0], "ALL"))) list = list.slice(1);
  const from = end !== -1 && isWord(tokens[end], "FROM") ? sources(tokens, end, tables) : new Map<string, Source>();

  const columns: ContractColumn[] = [];
  const unnamed: { expression: string }[] = [];
  for (const expression of splitTopLevel(list, 0)) {
    if (expression.length === 0) continue;
    const text = statement.slice(expression[0].start, expression[expression.length - 1].end);
    // `*` and `t.*`
    if (isSymbol(expression[expression.length - 1], "*") && (expression.length === 1 || (expression.length === 3 && isSymbol(expression[1], ".")))) {
      const qualifier = expression.length === 3 ? nameOf(expression[0])?.toLowerCase() : undefined;
      const expanded = [...from.entries()].filter(([alias]) => !qualifier || alias === qualifier);
      if (expanded.length === 0 || expanded.some(([, source]) => !source.columns)) {
        unnamed.push({ expression: text });
        continue;
      }
      for (const [, source] of expanded) {
        for (const column of source.columns!) columns.push(fromColumn(column.name, column, source));
      }
      continue;
    }
    const { body, alias } = splitAlias(expression);
    const reference = columnReference(body);
    if (reference) {
      const source = resolveSource(from, reference.qualifier, reference.column);
      const known = source?.columns?.find((column) => column.name.toLowerCase() === reference.column.toLowerCase());
      const name = alias ?? known?.name ?? reference.column;
      columns.push(known && source ? fromColumn(name, known, source) : { name, type: "unknown", nullable: null });
      continue;
    }
    if (alias) {
      columns.push({ name: alias, ...expressionType(body) });
      continue;
    }
    unnamed.push({ expression: text });
  }
  return { columns, unnamed };
}

type Source = { table: string | null; columns: SchemaColumn[] | null; nullable: boolean };

/** FROM and JOIN sources by the name a column qualifier uses, lower-cased. */
function sources(tokens: SqlToken[], from: number, tables: SchemaTables): Map<string, Source> {
  const result = new Map<string, Source>();
  const stop = new Set(["WHERE", "GROUP", "HAVING", "ORDER", "LIMIT", "UNION", "EXCEPT", "INTERSECT", "WINDOW"]);
  let index = from + 1;
  let leftJoined = false;
  while (index < tokens.length) {
    const token = tokens[index];
    if (token.depth !== 0) { index += 1; continue; }
    if (token.kind === "word" && stop.has(token.text.toUpperCase())) break;
    if (isSymbol(token, "(")) {
      // A subquery or a table-valued function: its columns are not read here.
      let close = index + 1;
      while (close < tokens.length && !(tokens[close].depth === 0 && isSymbol(tokens[close], ")"))) close += 1;
      const alias = aliasAfter(tokens, close + 1);
      const previous = tokens[index - 1];
      const fn = previous && previous.depth === 0 && previous.kind === "word" && !isJoinWord(previous) ? previous.text.toLowerCase() : null;
      if (fn) result.delete(fn);
      result.set((alias?.name ?? fn ?? `subquery${result.size}`).toLowerCase(), { table: null, columns: null, nullable: leftJoined });
      index = alias ? alias.next : close + 1;
      leftJoined = false;
      continue;
    }
    if (token.kind === "word" && isJoinWord(token)) {
      if (isWord(token, "LEFT") || isWord(token, "RIGHT") || isWord(token, "FULL")) leftJoined = true;
      index += 1;
      continue;
    }
    if (isSymbol(token, ",")) { index += 1; continue; }
    if (token.kind === "word" && (isWord(token, "ON") || isWord(token, "USING"))) {
      index += 1;
      while (index < tokens.length && !(tokens[index].depth === 0 && (isSymbol(tokens[index], ",") || (tokens[index].kind === "word" && (isJoinWord(tokens[index]) || stop.has((tokens[index] as { text: string }).text.toUpperCase())))))) index += 1;
      continue;
    }
    const name = nameOf(token);
    if (name) {
      let table = name;
      let next = index + 1;
      if (isSymbol(tokens[next], ".") && nameOf(tokens[next + 1])) { table = nameOf(tokens[next + 1])!; next += 2; }
      if (isSymbol(tokens[next], "(")) { index = next; continue; }
      const alias = aliasAfter(tokens, next);
      const columns = tables.get(table.toLowerCase()) ?? null;
      result.set((alias?.name ?? table).toLowerCase(), { table: columns ? table : null, columns, nullable: leftJoined });
      leftJoined = false;
      index = alias ? alias.next : next;
      continue;
    }
    index += 1;
  }
  return result;
}

const joinWords = new Set(["JOIN", "LEFT", "RIGHT", "FULL", "INNER", "OUTER", "CROSS", "NATURAL"]);
function isJoinWord(token: SqlToken): boolean {
  return token.kind === "word" && joinWords.has(token.text.toUpperCase());
}

const notAliases = new Set(["ON", "USING", "WHERE", "GROUP", "HAVING", "ORDER", "LIMIT", "UNION", "EXCEPT", "INTERSECT", "WINDOW", "INDEXED", "NOT", ...joinWords]);
function aliasAfter(tokens: SqlToken[], index: number): { name: string; next: number } | null {
  let at = index;
  if (tokens[at] && isWord(tokens[at], "AS")) at += 1;
  const token = tokens[at];
  if (!token || token.depth !== 0) return null;
  if (token.kind === "identifier") return { name: token.text, next: at + 1 };
  if (token.kind === "word" && !notAliases.has(token.text.toUpperCase())) return { name: token.text, next: at + 1 };
  return null;
}

function resolveSource(from: Map<string, Source>, qualifier: string | undefined, column: string): Source | undefined {
  if (qualifier) return from.get(qualifier.toLowerCase());
  const candidates = [...from.values()].filter((source) => source.columns?.some((known) => known.name.toLowerCase() === column.toLowerCase()));
  return candidates.length === 1 ? candidates[0] : undefined;
}

function fromColumn(name: string, column: SchemaColumn, source: Source): ContractColumn {
  return {
    name,
    type: column.type,
    nullable: column.nullable || source.nullable,
    ...(source.table ? { from: { table: source.table, column: column.name } } : {}),
  };
}

/** `name`, `t.name`, or a quoted form of either. */
function columnReference(tokens: SqlToken[]): { qualifier?: string; column: string } | null {
  if (tokens.length === 1) {
    const column = nameOf(tokens[0]);
    return column && !isLiteralWord(tokens[0]) ? { column } : null;
  }
  if (tokens.length === 3 && isSymbol(tokens[1], ".")) {
    const qualifier = nameOf(tokens[0]);
    const column = nameOf(tokens[2]);
    return qualifier && column ? { qualifier, column } : null;
  }
  return null;
}

function isLiteralWord(token: SqlToken): boolean {
  return token.kind === "word" && ["NULL", "TRUE", "FALSE", "CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME"].includes(token.text.toUpperCase());
}

/** Separates a trailing `AS alias` or bare alias from a result column. */
function splitAlias(tokens: SqlToken[]): { body: SqlToken[]; alias?: string } {
  const last = tokens[tokens.length - 1];
  const before = tokens[tokens.length - 2];
  if (tokens.length >= 3 && before && isWord(before, "AS") && (last.kind === "word" || last.kind === "identifier" || last.kind === "string")) {
    return { body: tokens.slice(0, -2), alias: aliasText(last) };
  }
  // `expr alias`: a name directly after a complete expression. `t.col` and
  // `count(*)` end in a name or `)`, so a bare word after them is an alias.
  if (tokens.length >= 2 && (last.kind === "word" || last.kind === "identifier") && before
    && !isSymbol(before, ".") && (before.kind !== "word" || isLiteralWord(before) || !isKeyword(before))) {
    if (last.kind === "word" && isKeyword(last)) return { body: tokens };
    return { body: tokens.slice(0, -1), alias: aliasText(last) };
  }
  return { body: tokens };
}

function aliasText(token: SqlToken): string | undefined {
  if (token.kind === "word" || token.kind === "identifier") return token.text;
  return undefined;
}

const keywords = new Set(["AND", "OR", "NOT", "IS", "IN", "LIKE", "GLOB", "BETWEEN", "CASE", "WHEN", "THEN", "ELSE", "END", "ESCAPE", "COLLATE", "DISTINCT", "CAST", "EXISTS", "NULL", "AS", "FILTER", "OVER", "ASC", "DESC"]);
function isKeyword(token: SqlToken): boolean {
  return token.kind === "word" && keywords.has(token.text.toUpperCase());
}

/** The type of an aliased expression, where its outermost function settles it. */
function expressionType(tokens: SqlToken[]): Pick<ContractColumn, "type" | "nullable"> {
  const head = tokens[0];
  const call = head?.kind === "word" && isSymbol(tokens[1], "(") && isSymbol(tokens[tokens.length - 1], ")") && tokens[tokens.length - 1].depth === head.depth
    && tokens.slice(2, -1).every((token) => token.depth > head.depth);
  if (call) {
    const name = head.text.toUpperCase();
    if (name === "COUNT") return { type: "integer", nullable: false };
    if (name === "TOTAL") return { type: "real", nullable: false };
    if (name === "AVG") return { type: "real", nullable: true };
    if (["LENGTH", "INSTR", "UNICODE"].includes(name)) return { type: "integer", nullable: null };
    if (["LOWER", "UPPER", "TRIM", "LTRIM", "RTRIM", "SUBSTR", "SUBSTRING", "REPLACE", "STRFTIME", "DATE", "TIME", "DATETIME", "GROUP_CONCAT", "PRINTF", "FORMAT", "HEX", "QUOTE", "TYPEOF"].includes(name)) return { type: "text", nullable: null };
  }
  if (tokens.length === 1) {
    if (tokens[0].kind === "string") return { type: "text", nullable: false };
    if (tokens[0].kind === "symbol" && /^\d+$/.test(tokens[0].text)) return { type: "integer", nullable: false };
    if (tokens[0].kind === "symbol" && /^[\d.]+(e\d+)?$/i.test(tokens[0].text)) return { type: "real", nullable: false };
  }
  return { type: "unknown", nullable: null };
}

/** Splits tokens on commas at `depth`, ignoring commas inside parentheses. Stops at the first `)` that closes `depth`. */
function splitTopLevel(tokens: SqlToken[], depth: number): SqlToken[][] {
  const parts: SqlToken[][] = [[]];
  for (const token of tokens) {
    if (token.depth < depth) break;
    if (token.depth === depth && isSymbol(token, ",")) { parts.push([]); continue; }
    parts[parts.length - 1].push(token);
  }
  return parts;
}

function isWord(token: SqlToken | undefined, word: string): boolean {
  return token?.kind === "word" && token.text.toUpperCase() === word;
}

function isSymbol(token: SqlToken | undefined, symbol: string): boolean {
  return token?.kind === "symbol" && token.text === symbol;
}

function nameOf(token: SqlToken | undefined): string | undefined {
  if (!token) return undefined;
  if (token.kind === "identifier") return token.text;
  if (token.kind === "word") return token.text;
  return undefined;
}

// --- Markdown -------------------------------------------------------------------

/** The contract as Markdown for a person. The JSON is the authority. */
export function renderActionContractMarkdown(contract: ActionContract): string {
  const lines: string[] = [];
  const title = contract.project ? `Action Contract for ${contract.project.name}` : "Action Contract";
  lines.push(`# ${title}`, "");
  lines.push(`Generated from actions.json and schema.sql. ${ACTION_CONTRACT_FILE} is the machine-readable form and ${ACTION_CONTRACT_SCHEMA_FILE} its schema. Do not edit either; change the Capsule and generate them again.`, "");
  lines.push(`${contract.actions.length} action${contract.actions.length === 1 ? "" : "s"}. Every action runs on the runtime endpoint \`${RUNTIME.endpoint}\` as the \`${RUNTIME.tool}\` tool, which \`${RUNTIME.sdk}(name, input)\` calls.`, "");
  for (const note of contract.notes.slice(1)) lines.push(`- ${note}`);
  lines.push("");
  lines.push("| Action | Kind | Access | HTTP |", "| --- | --- | --- | --- |");
  for (const action of contract.actions) {
    lines.push(`| \`${action.name}\` | ${action.kind} | ${action.access} | ${action.http ? `\`${action.http.method} ${action.http.path}\`` : ""} |`);
  }
  lines.push("");
  for (const action of contract.actions) {
    lines.push(`## ${action.name}`, "");
    lines.push(`A ${action.kind} declared ${action.access}.${action.actorBound ? " It binds the caller's ID, so a caller without one is refused." : ""}`, "");
    lines.push("```sql", action.statement, "```", "");
    lines.push("### Input", "");
    if (action.inputs.length === 0) lines.push("None. Send `{}`.", "");
    else {
      lines.push("| Key | Type | Required | Limit |", "| --- | --- | --- | --- |");
      for (const input of action.inputs) {
        lines.push(`| \`${input.key}\` | ${input.type}${input.nullable ? " or null" : ""} | ${input.required ? "yes" : "no"} | ${input.maxLength !== undefined ? `${input.maxLength} UTF-16 code units` : ""} |`);
      }
      lines.push("");
    }
    lines.push("### Output", "");
    if (action.output.kind === "rows") {
      lines.push("An array of row objects.", "");
      if (action.output.columns.length > 0) {
        lines.push("| Column | Type | Nullable | Reads |", "| --- | --- | --- | --- |");
        for (const column of action.output.columns) {
          lines.push(`| \`${column.name}\` | ${column.type} | ${column.nullable === null ? "unknown" : column.nullable ? "yes" : "no"} | ${column.from ? `${column.from.table}.${column.from.column}` : ""} |`);
        }
        lines.push("");
      }
      for (const column of action.output.unnamed) {
        lines.push(`The column \`${column.expression}\` has no name. Its key is whatever the backend prints for the expression, so a second backend need not match it. Alias it with AS.`, "");
      }
    } else if (action.output.kind === "changes") {
      lines.push("`{ changes, lastInsertId }`, both integers.", "");
      if (action.guard) lines.push(`When the statement changes no row the call is refused with \`${action.guard}\` instead.`, "");
      if (action.schedule) lines.push(`The Kernel's timer also runs it ${"every" in action.schedule ? `every ${action.schedule.every}` : `daily at ${action.schedule.daily} UTC`}, with no caller: an \`actor\` binding is null and the guard does not apply. The owner reads each run under \`scheduled\` with \`usage_read\`.`, "");
    } else {
      lines.push(`\`{ "recorded": true }\`. It runs no SQL and records the Signal \`${action.output.signal}\`.`, "");
    }
    if (action.signal) {
      const { name, properties } = action.signal;
      lines.push("### Signal", "");
      const named = properties.map((property) => property === "actor" ? "`actor` (the app user's ID, the API key's ID, or null for a visitor or a signed request)" : `\`${property}\``);
      lines.push(`Records \`${name}\`${named.length ? ` with ${named.join(", ")}` : ""}${action.kind === "mutation" ? " after the statement changes a row" : " on every call the bindings accept"}. A refused call records none.`, "");
    }
    lines.push("### Callers", "");
    lines.push(`Runtime endpoint: ${action.runtime.callers.length ? action.runtime.callers.map(describeCaller).join("; ") : "none; the action answers only on its route"}.`, "");
    if (action.http) {
      const http = action.http;
      lines.push(`HTTP route \`${http.method} ${http.path}\` on the Deployment host. Input is ${http.input}. It answers \`{ "result": ... }\` with the same output, or \`{ "error": "<message>" }\` with a status.`, "");
      if (http.signature) {
        lines.push(`A signed request carries \`${http.signature.header}: ${http.signature.scheme}\`. The secret is named \`${http.signature.secret}\` and set by the owner; it is never in this file. \`t\` must be within ${http.signature.windowSeconds} seconds of the server's clock, and each signature is accepted once.`, "");
      }
      lines.push(`Callers: ${http.callers.map(describeCaller).join("; ")}.`, "");
      lines.push(http.onlyDoor === "always"
        ? "The route is the action's only door: the runtime endpoint refuses it."
        : "On a private Deployment the route takes an API key or a session and is the action's only door: the runtime endpoint refuses it there.", "");
    }
    lines.push("### Errors", "");
    lines.push("| Message | Status | Seen on | When |", "| --- | --- | --- | --- |");
    for (const error of action.errors) {
      lines.push(`| \`${error.message}\` | ${error.status ?? ""} | ${error.via} | ${escapeCell(error.when)} |`);
    }
    lines.push("");
    lines.push("### Limits", "");
    const limits = action.limits;
    if (action.kind === "query") {
      lines.push(`- ${limits.bound === "limit" ? `LIMIT ${limits.limit ?? "(a parameter)"} at the top of the statement` : "returns one row, so it needs no LIMIT"}; at most ${limits.maxRows} rows and ${limits.maxResultBytes / 1024 / 1024} MiB of JSON in any case.`);
    }
    lines.push(`- ${limits.statementSeconds} seconds per statement.`);
    for (const rate of limits.rate) lines.push(`- ${rate.calls} calls per ${rate.seconds} seconds for a ${rate.caller}.`);
    if (limits.maxRequestBytes) lines.push(`- A route body of at most ${limits.maxRequestBytes / 1024} KiB.`);
    lines.push("");
  }
  lines.push("## Any action", "");
  lines.push("| Message | Status | Seen on | When |", "| --- | --- | --- | --- |");
  for (const error of contract.errors) lines.push(`| \`${error.message}\` | ${error.status ?? ""} | ${error.via} | ${escapeCell(error.when)} |`);
  lines.push("");
  return lines.join("\n");
}

function describeCaller(caller: ContractCaller): string {
  return `${caller.kind} (${caller.proof}; actor ${caller.actor})`;
}

function escapeCell(text: string): string {
  return text.replaceAll("|", "\\|");
}

/** Pretty JSON with a trailing newline, the form written to disk and into the export. */
export function serializeActionContract(contract: ActionContract): string {
  return `${JSON.stringify({ $schema: `./${ACTION_CONTRACT_SCHEMA_FILE}`, ...contract }, null, 2)}\n`;
}

// --- Schema check ---------------------------------------------------------------

/**
 * Checks a value against action-contract.schema.json. It reads the subset of
 * JSON Schema that file uses: type, const, enum, properties, required,
 * additionalProperties, items, minimum, oneOf and $ref to $defs. Returns the
 * problems, empty when the value conforms.
 */
export function checkActionContract(value: unknown): string[] {
  return checkSchema(value, actionContractSchema, actionContractSchema, "$");
}

type JsonSchema = {
  $ref?: string;
  $defs?: Record<string, JsonSchema>;
  type?: string | string[];
  const?: unknown;
  enum?: unknown[];
  properties?: Record<string, JsonSchema>;
  required?: string[];
  additionalProperties?: boolean;
  items?: JsonSchema;
  minimum?: number;
  minItems?: number;
  maxItems?: number;
  oneOf?: JsonSchema[];
  [key: string]: unknown;
};

function checkSchema(value: unknown, schema: JsonSchema, root: JsonSchema, at: string): string[] {
  if (schema.$ref) {
    const name = schema.$ref.replace(/^#\/\$defs\//, "");
    const target = root.$defs?.[name];
    if (!target) return [`${at}: unknown $ref ${schema.$ref}`];
    return checkSchema(value, target, root, at);
  }
  if (schema.oneOf) {
    const matching = schema.oneOf.filter((option) => checkSchema(value, option, root, at).length === 0);
    return matching.length === 1 ? [] : [`${at}: matches ${matching.length} of the allowed shapes, not exactly one`];
  }
  if (schema.const !== undefined && value !== schema.const) return [`${at}: must be ${JSON.stringify(schema.const)}`];
  if (schema.enum && !schema.enum.includes(value as never)) return [`${at}: must be one of ${schema.enum.map((item) => JSON.stringify(item)).join(", ")}`];
  if (schema.type) {
    const types = Array.isArray(schema.type) ? schema.type : [schema.type];
    if (!types.some((type) => jsonType(value, type))) return [`${at}: must be ${types.join(" or ")}`];
  }
  const problems: string[] = [];
  if (typeof value === "number" && schema.minimum !== undefined && value < schema.minimum) problems.push(`${at}: must be at least ${schema.minimum}`);
  if (Array.isArray(value)) {
    if (schema.minItems !== undefined && value.length < schema.minItems) problems.push(`${at}: needs at least ${schema.minItems} items`);
    if (schema.maxItems !== undefined && value.length > schema.maxItems) problems.push(`${at}: allows at most ${schema.maxItems} items`);
    if (schema.items) value.forEach((item, index) => problems.push(...checkSchema(item, schema.items!, root, `${at}[${index}]`)));
  }
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const record = value as Record<string, unknown>;
    for (const key of schema.required ?? []) if (!(key in record)) problems.push(`${at}: missing ${key}`);
    for (const [key, item] of Object.entries(record)) {
      const property = schema.properties?.[key];
      if (property) problems.push(...checkSchema(item, property, root, `${at}.${key}`));
      else if (schema.additionalProperties === false) problems.push(`${at}: unexpected ${key}`);
    }
  }
  return problems;
}

function jsonType(value: unknown, type: string): boolean {
  switch (type) {
    case "null": return value === null;
    case "boolean": return typeof value === "boolean";
    case "string": return typeof value === "string";
    case "number": return typeof value === "number" && Number.isFinite(value);
    case "integer": return Number.isInteger(value);
    case "array": return Array.isArray(value);
    case "object": return value !== null && typeof value === "object" && !Array.isArray(value);
    default: return false;
  }
}

/** The actions.json program a contract describes, for the round-trip check. */
export function programFromContract(contract: ActionContract): ServerActionProgram {
  const actions: ServerActionProgram["actions"] = {};
  for (const action of contract.actions) {
    const bindings = action.bindings.map((binding) => {
      if (binding.source !== "input") return binding;
      const input = action.inputs.find((candidate) => candidate.key === binding.key)!;
      return {
        source: "input" as const,
        key: input.key,
        type: input.type,
        ...(input.maxLength !== undefined ? { maxLength: input.maxLength } : {}),
        ...(input.nullable ? { nullable: true } : {}),
      };
    }) as ServerActionDefinition["bindings"];
    actions[action.name] = {
      name: action.name,
      kind: action.kind,
      access: action.access,
      statement: action.statement,
      bindings,
      ...(action.signal ? { signal: { name: action.signal.name, properties: [...action.signal.properties] } } : {}),
      ...(action.guard ? { guard: action.guard } : {}),
      ...(action.schedule ? { schedule: { ...action.schedule } } : {}),
      ...(action.http ? {
        http: {
          method: action.http.method,
          path: action.http.path,
          ...(action.http.signature ? { signature: { header: action.http.signature.header, secret: action.http.signature.secret } } : {}),
          ...(action.http.declaredCallers ? { callers: [...action.http.declaredCallers] } : {}),
        },
      } : {}),
    };
  }
  return { version: 1, actions };
}
