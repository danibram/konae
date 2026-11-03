import type { RuntimeActionDeclaration } from "../deployments/manifest";
import { unboundedQueryReason } from "../lib/query-bound";
import { assertRuntimeSql, validateRuntimeParams } from "../runtime/security";
import { parseRouteFields, parseRouteSignature, type RouteSignature } from "./route-declaration";
import { refusal } from "./refusal";
import { assertActionStatement } from "./sql-rules";
import { sqlTokens } from "../lib/sql-tokens";
import { assertCallTargets, assertNoCallKeys, parseCall, type CallDeclaration } from "./calls";

type ActionBinding =
  | { source: "input"; key: string; type: "string"; maxLength?: number; nullable?: boolean }
  | { source: "input"; key: string; type: "number" | "boolean"; nullable?: boolean }
  | { source: "actor"; field: "id" }
  | { source: "kernel"; field: "now" }
  | { source: "literal"; value: string | number | null };

/** Who may call a route: a signed sender, a program with an API key, a person with a session. */
export type RouteCaller = "signature" | "key" | "session";

/** An action's HTTP route on its Deployment host. See docs/mcp.md, "HTTP routes". */
export type HttpBinding = {
  method: "GET" | "POST";
  path: string;
  /** `secret` names a value the owner sets with `secret_set`; it is never in actions.json. See route-declaration.ts for a provider's scheme. */
  signature?: RouteSignature;
  /**
   * The route's callers. Without it a signed route takes the signature or a
   * key, an unsigned route on a private Deployment a key or a session, and an
   * unsigned route on a public Deployment anyone. See `routeCallers`.
   */
  callers?: RouteCaller[];
  /** Input keys and the JSON path each reads from a POST body; other body keys are left out (ADR-0018). */
  fields?: Record<string, string>;
};

const ROUTE_CALLERS: readonly RouteCaller[] = ["signature", "key", "session"];

/**
 * The callers a route accepts on a Deployment of the given visibility, or
 * null when it answers anyone. An action whose route does not answer anyone
 * answers only there, never through actions_invoke (ADR-0011, update of
 * 2026-11-06). kernel-rs/kernel/src/http.rs `route_callers` is the same rule.
 */
export function routeCallers(http: HttpBinding, visibility: "public" | "private"): RouteCaller[] | null {
  if (http.callers) return http.callers;
  if (http.signature) return ["signature", "key"];
  return visibility === "private" ? ["key", "session"] : null;
}

/**
 * A Signal the action records when it succeeds. Its properties name input
 * bindings. See docs/adr/0010-kernel-owns-usage-and-signals.md.
 */
export type SignalDeclaration = { name: string; properties: string[] };

/**
 * The email a mutation queues when its statement changes a row. It goes to
 * the Project Owner or to the calling account, never to an address from the
 * input. See docs/adr/0016-a-mutation-may-notify-the-owner-or-the-actor.md.
 */
export type NotifyDeclaration = {
  to: "owner" | "actor" | { column: string };
  /** Columns of the changed rows that {changes} lists (ADR-0021). */
  changes?: string[];
  subject: string;
  body: string;
};

export type ServerActionDefinition = RuntimeActionDeclaration & {
  /** Empty for a `"kind": "signal"` action, which runs no SQL. */
  statement: string;
  bindings: ActionBinding[];
  http?: HttpBinding;
  signal?: SignalDeclaration;
  /**
   * The refusal a mutation answers with when its statement changes no row,
   * instead of { changes: 0 }. See docs/mcp.md, "Guards".
   */
  guard?: string;
  /** The email the mutation queues when it changes a row. See docs/mcp.md, "Notices". */
  notify?: NotifyDeclaration;
  /**
   * When the Kernel's timer runs a mutation that takes no input: every
   * `<n>m` or `<n>h` from 5 minutes to 24 hours, or daily at `HH:MM` UTC.
   * See docs/mcp.md, "Scheduled work".
   */
  schedule?: ScheduleDeclaration;
  /** An action of `"kind": "call"`. See docs/mcp.md, "Calls between apps". */
  call?: CallDeclaration;
};

export type ScheduleDeclaration = { every: string } | { daily: string };

export type ServerActionProgram = {
  version: 1;
  actions: Record<string, ServerActionDefinition>;
};

export function parseServerActionSource(source: string): ServerActionProgram {
  let candidate: unknown;
  try {
    candidate = JSON.parse(source);
  } catch {
    throw new Error("actions.json must contain valid JSON.");
  }
  if (!candidate || typeof candidate !== "object") throw new Error("actions.json must contain an object.");
  const record = candidate as Record<string, unknown>;
  if (record.version !== 1 || !record.actions || typeof record.actions !== "object" || Array.isArray(record.actions)) {
    throw new Error("actions.json must use server action format version 1.");
  }

  // The same checks, in the same order and with the same messages, as
  // kernel-rs/kernel/src/action_shape.rs `check_program`. Sorted names make both doors
  // name the same action when two are wrong.
  const declared = record.actions as Record<string, unknown>;
  if (Object.keys(declared).length > MAX_ACTIONS) throw refusal("action.count-over", `A project may declare at most ${MAX_ACTIONS} actions.`);
  const parsed = new Map(Object.keys(declared).sort().map((name) => [name, parseAction(name, declared[name])]));
  // The program keeps the order actions.json writes them in.
  const actions: Record<string, ServerActionDefinition> = {};
  for (const name of Object.keys(declared)) actions[name] = parsed.get(name)!;
  assertDistinctRoutes(actions);
  assertCallTargets(actions);
  return { version: 1, actions };
}

// kernel-rs/kernel/src/action-cases.json holds the shape cases both test suites run.
const MAX_ACTIONS = 100;
const MAX_ACTION_NAME = 100;
const ACTION_NAME = /^[a-z][a-z0-9]*(?:\.[a-z][a-z0-9]*)+$/;
const MAX_STATEMENT_BYTES = 20_000;
const MAX_BINDINGS = 100;

export function actionDeclarations(program: ServerActionProgram): RuntimeActionDeclaration[] {
  return Object.values(program.actions).map(({ name, kind, access }) => ({ name, kind, access }));
}

/** Who calls: a person's or an API key's ID, or null for a visitor. */
export type ResolveContext = { actorId: string | null; now?: number };

export function resolveServerAction(
  program: ServerActionProgram,
  name: string,
  input: unknown,
  context: ResolveContext,
) {
  const action = program.actions[name];
  if (!action) throw new Error("Server action not declared.");
  const values = input === undefined ? {} : input;
  if (!values || typeof values !== "object" || Array.isArray(values)) throw new Error("Action input must be an object.");
  const inputRecord = values as Record<string, unknown>;
  const acceptedKeys = new Set(action.bindings.flatMap((binding) => binding.source === "input" ? [binding.key] : []));
  const unexpected = Object.keys(inputRecord).find((key) => !acceptedKeys.has(key));
  if (unexpected) throw new Error(`Unexpected action input: ${unexpected}`);

  const params = action.bindings.map((binding) => resolveBinding(binding, inputRecord, context));
  validateRuntimeParams(params);
  return { action, params };
}

/**
 * The top-level keys each kind takes. A key outside its kind's list is refused
 * after the action's other checks, so a key that has its own refusal, such as a
 * guard on a query, keeps it. `statement` on a signal action or a call may only
 * be empty. kernel-rs/kernel/src/action_shape.rs `check_keys` holds the same lists.
 */
const ACTION_KEYS: Record<ServerActionDefinition["kind"], readonly string[]> = {
  query: ["kind", "access", "statement", "bindings", "http"],
  mutation: ["kind", "access", "statement", "bindings", "http", "signal", "guard", "notify", "schedule"],
  signal: ["kind", "access", "statement", "bindings", "signal"],
  call: ["kind", "access", "statement", "bindings", "url", "method", "body", "headers", "auth", "then"],
};

function parseAction(name: string, value: unknown): ServerActionDefinition {
  const action = parseActionBody(name, value);
  // A key set to null counts as absent, as the Kernel reads it.
  const record = value as Record<string, unknown>;
  const allowed = ACTION_KEYS[action.kind];
  const extra = Object.keys(record).sort().find((key) => record[key] != null && !allowed.includes(key));
  if (extra !== undefined) {
    throw refusal("action.key-unknown", `Action ${name} has a key ${extra} that a ${action.kind} action does not take.`);
  }
  return action;
}

function parseActionBody(name: string, value: unknown): ServerActionDefinition {
  if (name.length > MAX_ACTION_NAME || !ACTION_NAME.test(name)) throw refusal("action.name-invalid", `Invalid server action name: ${name}`);
  if (!value || typeof value !== "object") throw new Error(`Invalid server action: ${name}`);
  const candidate = value as Record<string, unknown>;
  if (candidate.access === "granted") {
    // Reserved for per-grant access; the Kernel refuses to deploy it until grants exist.
    throw refusal("action.access-unsupported", `Action ${name} uses "access": "granted", which the Kernel does not support yet. Use "anonymous" or "authenticated".`);
  }
  const access = candidate.access === "anonymous" || candidate.access === "authenticated" ? candidate.access : null;
  if (!access) throw refusal("action.access-invalid", `Action ${name} has an invalid access. Use "anonymous" or "authenticated".`);
  const kind = candidate.kind === "query" || candidate.kind === "mutation" || candidate.kind === "signal" || candidate.kind === "call" ? candidate.kind : null;
  if (!kind) throw refusal("action.kind-invalid", `Action ${name} has an invalid kind. Use "query", "mutation", "signal" or "call".`);
  if (!Array.isArray(candidate.bindings)) throw new Error(`Action ${name} requires a bindings array.`);
  const bindings = candidate.bindings.map((binding, index) => parseBinding(name, index, binding));
  if (kind !== "signal" && kind !== "call" && typeof candidate.statement !== "string") throw new Error(`Invalid server action: ${name}`);
  const statementBytes = typeof candidate.statement === "string" ? new TextEncoder().encode(candidate.statement).length : 0;
  if (statementBytes > MAX_STATEMENT_BYTES || bindings.length > MAX_BINDINGS) throw refusal("action.limits-exceeded", `Action ${name} exceeds statement or binding limits.`);
  // A call runs no SQL; the Kernel's worker sends it (kernel-rs/kernel/src/calls.rs).
  if (kind === "call") return { name, kind, access, statement: "", bindings, call: parseCall(name, bindings, candidate) };
  assertNoCallKeys(name, kind, candidate);
  if (kind === "signal") {
    if (candidate.statement !== undefined && !(typeof candidate.statement === "string" && !candidate.statement.trim())) {
      throw refusal("signal.statement", `Action ${name} is a signal action, so it runs no SQL and cannot declare a statement.`);
    }
    const signal = parseSignal(name, kind, bindings, candidate.signal);
    const http = candidate.http === undefined ? undefined : parseHttpBinding(name, kind, access, candidate.http, bindings);
    parseGuard(name, kind, candidate.guard);
    parseNotify(name, kind, bindings, candidate.notify);
    assertNoSubscribe(name, candidate);
    parseSchedule(name, kind, bindings, candidate.schedule);
    return { name, kind, access, statement: "", bindings, ...(signal ? { signal } : {}), ...(http ? { http } : {}) };
  }
  const statement = candidate.statement as string;
  assertActionStatement(name, kind === "query" ? "query" : "mutation", statement);
  assertRuntimeSql(kind === "query" ? "query" : "execute", statement);
  if (kind === "query") {
    const reason = unboundedQueryReason(statement);
    if (reason) {
      throw refusal("query.limit-missing", `Action ${name} ${reason} Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.`);
    }
  }
  const signal = parseSignal(name, kind, bindings, candidate.signal);
  const http = candidate.http === undefined ? undefined : parseHttpBinding(name, kind, access, candidate.http, bindings);
  const guard = parseGuard(name, kind, candidate.guard);
  assertNoSubscribe(name, candidate);
  const schedule = parseSchedule(name, kind, bindings, candidate.schedule);
  const notify = parseNotify(name, kind, bindings, candidate.notify);
  return {
    name, kind, access, statement: statement.trim(), bindings,
    ...(signal ? { signal } : {}), ...(http ? { http } : {}), ...(guard ? { guard } : {}), ...(notify ? { notify } : {}), ...(schedule ? { schedule } : {}),
  };
}

// The same checks, in the same order and with the same messages, as
// kernel-rs/kernel/src/schedule.rs `check_subscribe` and `declaration`.
// kernel-rs/kernel/src/schedule-cases.json holds the cases both test suites run.
const MIN_SCHEDULE_MINUTES = 5;
const MAX_SCHEDULE_MINUTES = 24 * 60;

function assertNoSubscribe(name: string, candidate: Record<string, unknown>) {
  if (candidate.subscribe !== undefined) {
    throw refusal("action.subscribe-unsupported", `Action ${name} declares subscribe, but the Kernel pushes no updates to a page. Poll a query that reads rows after a cursor instead.`);
  }
}

function parseSchedule(name: string, kind: "query" | "mutation" | "signal", bindings: ActionBinding[], value: unknown): ScheduleDeclaration | undefined {
  if (value === undefined) return undefined;
  if (kind !== "mutation") throw refusal("schedule.not-mutation", `Action ${name} is a ${kind === "query" ? "query" : "signal action"}, so it cannot declare a schedule. Declare it on a mutation.`);
  if (bindings.some((binding) => binding.source === "input")) {
    throw refusal("schedule.input", `Action ${name} declares a schedule, so it cannot take input. A scheduled run has no caller to supply it; bind only actor, kernel and literal values.`);
  }
  const invalid = `Action ${name} has an invalid schedule. Declare "every" with minutes or hours, such as "15m" or "6h", or "daily" with a UTC time, such as "02:00".`;
  if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).length !== 1) throw refusal("schedule.invalid", invalid);
  const [key, text] = Object.entries(value)[0];
  if (typeof text !== "string") throw refusal("schedule.invalid", invalid);
  if (key === "every") {
    const match = /^([1-9][0-9]{0,4})([mh])$/.exec(text);
    if (!match) throw refusal("schedule.invalid", invalid);
    const minutes = Number(match[1]) * (match[2] === "h" ? 60 : 1);
    if (minutes < MIN_SCHEDULE_MINUTES) throw refusal("schedule.too-often", `Action ${name} runs every ${text}, more often than every 5 minutes. Use "5m" or longer.`);
    if (minutes > MAX_SCHEDULE_MINUTES) throw refusal("schedule.too-rare", `Action ${name} runs every ${text}, less often than once a day. Use "daily" or a period of 24 hours or less.`);
    return { every: text };
  }
  if (key === "daily") {
    const match = /^([0-9]{2}):([0-9]{2})$/.exec(text);
    if (!match || Number(match[1]) > 23 || Number(match[2]) > 59) throw refusal("schedule.daily-invalid", `Action ${name} has an invalid daily time. Use HH:MM in UTC, from 00:00 to 23:59.`);
    return { daily: text };
  }
  throw refusal("schedule.invalid", invalid);
}

// The same checks and messages as kernel-rs/kernel/src/guard.rs `declaration`.
// kernel-rs/kernel/src/guard-cases.json holds the cases both test suites run.
const MAX_GUARD_LENGTH = 200;

function parseGuard(name: string, kind: "query" | "mutation" | "signal", value: unknown): string | undefined {
  if (value === undefined) return undefined;
  if (kind !== "mutation") throw refusal("guard.not-mutation", `Action ${name} is a ${kind === "query" ? "query" : "signal action"}, so it cannot declare a guard. Declare it on a mutation.`);
  // Characters, not bytes or UTF-16 units, so both doors count an accented letter once.
  if (typeof value !== "string" || !value.trim() || [...value].length > MAX_GUARD_LENGTH || /[\u0000-\u001f\u007f]/.test(value)) {
    throw refusal("guard.invalid", `Action ${name} has an invalid guard. Use a message of 1 to ${MAX_GUARD_LENGTH} characters on one line.`);
  }
  return value;
}

// The same checks, in the same order and with the same messages, as
// kernel-rs/kernel/src/notify.rs `declaration`. kernel-rs/kernel/src/notify-cases.json holds
// the cases both test suites run.
const MAX_NOTIFY_SUBJECT = 120;
const MAX_NOTIFY_BODY = 2000;
const MAX_NOTIFY_CHANGES = 8;
/** A SQL name the Kernel can quote. */
const NOTIFY_COLUMN = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;

function parseNotify(name: string, kind: "query" | "mutation" | "signal", bindings: ActionBinding[], value: unknown): NotifyDeclaration | undefined {
  if (value === undefined) return undefined;
  if (kind !== "mutation") throw refusal("notify.not-mutation", `Action ${name} is a ${kind === "query" ? "query" : "signal action"}, so it cannot declare notify. Declare it on a mutation.`);
  const record = value as Record<string, unknown> | null;
  if (!record || typeof record !== "object" || Array.isArray(record)
    || Object.keys(record).some((key) => !["to", "subject", "body", "changes"].includes(key))
    || typeof record.subject !== "string" || typeof record.body !== "string") {
    throw refusal("notify.invalid", `Action ${name} has an invalid notify declaration. Give "to", "subject", "body" and, to list changed rows, "changes", and nothing else.`);
  }
  const to = notifyRecipient(record.to);
  if (!to) throw refusal("notify.recipient-invalid", `Action ${name} has an invalid notify recipient. Use "owner", "actor" or {"column": "<name>"} for a column of account IDs.`);
  const changes = record.changes;
  if (changes !== undefined && (!Array.isArray(changes) || changes.length < 1 || changes.length > MAX_NOTIFY_CHANGES
    || changes.some((column, index) => typeof column !== "string" || !NOTIFY_COLUMN.test(column) || changes.indexOf(column) !== index))) {
    throw refusal("notify.changes-invalid", `Action ${name} has an invalid notify changes list. Name 1 to ${MAX_NOTIFY_CHANGES} columns of the changed table, each once, in letters, digits and underscores.`);
  }
  // Characters, not bytes or UTF-16 units, so both doors count an accented letter once.
  const { subject, body } = record;
  if (!subject.trim() || [...subject].length > MAX_NOTIFY_SUBJECT || /[\u0000-\u001f\u007f]/.test(subject)) {
    throw refusal("notify.subject-invalid", `Action ${name} has an invalid notify subject. Use 1 to ${MAX_NOTIFY_SUBJECT} characters on one line.`);
  }
  if (!body.trim() || [...body].length > MAX_NOTIFY_BODY || /[\u0000-\u0009\u000b-\u001f\u007f]/.test(body)) {
    throw refusal("notify.body-invalid", `Action ${name} has an invalid notify body. Use 1 to ${MAX_NOTIFY_BODY} characters, with line breaks and no other control characters.`);
  }
  const inputs = new Set(bindings.flatMap((binding) => binding.source === "input" ? [binding.key] : []));
  const inSubject = assertPlaceholders(name, "subject", subject, inputs);
  const inBody = assertPlaceholders(name, "body", body, inputs);
  if (inSubject.has("changes")) {
    throw refusal("notify.changes-in-subject", `Action ${name} puts {changes} in its notify subject. Put {changes} in the body, and {changes.count} in the subject.`);
  }
  if (inBody.has("changes") && changes === undefined) {
    throw refusal("notify.changes-undeclared", `Action ${name} uses {changes} in its notify body without a changes list. Name the columns to show in notify "changes".`);
  }
  return { to, ...(changes ? { changes: changes as string[] } : {}), subject, body };
}

function notifyRecipient(value: unknown): NotifyDeclaration["to"] | undefined {
  if (value === "owner" || value === "actor") return value;
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record);
  if (keys.length !== 1 || typeof record.column !== "string" || !NOTIFY_COLUMN.test(record.column)) return undefined;
  return { column: record.column };
}

/**
 * The statement with the Kernel's own RETURNING clause, as
 * kernel-rs/kernel/src/notify.rs `with_returning` writes it: a trailing ";" and what
 * follows are cut, and each column is in brackets, because SQLite reads an
 * unknown "name" in double quotes as a string.
 */
export function notifyReturningStatement(statement: string, columns: readonly string[]): string {
  const end = sqlTokens(statement).find((token) => token.kind === "symbol" && token.text === ";")?.start ?? statement.length;
  return `${statement.slice(0, end).trimEnd()}\nRETURNING ${columns.map((column) => `[${column}]`).join(", ")}`;
}

/** The refusal for a notify column the statement cannot return (kernel-rs/kernel/src/notify-column-cases.json). */
export function notifyColumnRefusal(name: string) {
  return refusal("notify.column-unknown", `Action ${name} names a notify column its statement cannot return. Name columns of the table the statement changes.`);
}

/**
 * The columns the Kernel appends as RETURNING so a notice can list and
 * address the changed rows: the changes list, then the recipient column.
 * kernel-rs/kernel/src/notify.rs `Declaration::returning` is the twin.
 */
export function notifyReturning(notify: NotifyDeclaration | undefined): string[] {
  if (!notify) return [];
  const columns = [...(notify.changes ?? [])];
  if (typeof notify.to === "object" && !columns.includes(notify.to.column)) columns.push(notify.to.column);
  return columns;
}

const NOTIFY_PLACEHOLDERS = new Set(["actor.id", "kernel.now", "changes", "changes.count"]);

/** Every "{" opens a placeholder the next "}" closes; a "}" on its own is text. Returns the placeholders it names. */
function assertPlaceholders(name: string, field: "subject" | "body", template: string, inputs: Set<string>): Set<string> {
  const named = new Set<string>();
  let rest = template;
  for (let open = rest.indexOf("{"); open >= 0; open = rest.indexOf("{")) {
    const close = rest.indexOf("}", open);
    if (close < 0) throw refusal("notify.placeholder-unclosed", `Action ${name} has an unclosed placeholder in its notify ${field}. Close it with "}".`);
    const token = rest.slice(open + 1, close);
    const known = NOTIFY_PLACEHOLDERS.has(token) || (token.startsWith("input.") && inputs.has(token.slice("input.".length)));
    if (!known) {
      throw refusal("notify.placeholder-unknown", `Action ${name} names an unknown placeholder {${token}} in its notify ${field}. Use {input.<key>} for one of its input bindings, {actor.id}, {kernel.now}, {changes} or {changes.count}.`);
    }
    named.add(token);
    rest = rest.slice(close + 1);
  }
  return named;
}

// The same checks, in the same order and with the same messages, as
// kernel-rs/kernel/src/usage.rs `declaration`. kernel-rs/kernel/src/signal-cases.json holds
// the cases both test suites run.
const SIGNAL_NAME = /^[a-z][a-z0-9]*(?:\.[a-z][a-z0-9]*)+$/;
const MAX_SIGNAL_PROPERTIES = 8;
/** The property that records the caller's ID: a user's, a key's, or null for a visitor or a signed request. */
const SIGNAL_ACTOR = "actor";

function parseSignal(name: string, kind: "query" | "mutation" | "signal", bindings: ActionBinding[], value: unknown): SignalDeclaration | undefined {
  if (value === undefined) {
    if (kind === "signal") throw refusal("signal.missing", `Action ${name} is a signal action, so it must declare a signal.`);
    return undefined;
  }
  if (kind === "query") {
    throw refusal("signal.on-query", `Action ${name} is a query, so it cannot declare a signal. Declare it on a mutation or on a "kind": "signal" action.`);
  }
  const invalid = `Action ${name} has an invalid signal declaration.`;
  if (!value || typeof value !== "object" || Array.isArray(value)) throw refusal("signal.invalid", invalid);
  const record = value as Record<string, unknown>;
  if (Object.keys(record).some((key) => key !== "name" && key !== "properties")) throw refusal("signal.invalid", invalid);
  if (typeof record.name !== "string") throw refusal("signal.invalid", invalid);
  if (record.name.length > 64 || !SIGNAL_NAME.test(record.name)) {
    throw refusal("signal.name-invalid", `Action ${name} names an invalid signal. Use dot-separated lowercase words, such as "order.placed", up to 64 characters.`);
  }
  const properties = record.properties === undefined ? [] : record.properties;
  if (!Array.isArray(properties) || properties.some((key) => typeof key !== "string")) throw refusal("signal.invalid", invalid);
  if (properties.length > MAX_SIGNAL_PROPERTIES) throw refusal("signal.properties-too-many", `Action ${name} names more than ${MAX_SIGNAL_PROPERTIES} signal properties.`);
  const inputs = new Set(bindings.flatMap((binding) => binding.source === "input" ? [binding.key] : []));
  const seen = new Set<string>();
  for (const key of properties as string[]) {
    if (seen.has(key)) throw refusal("signal.property-twice", `Action ${name} names signal property ${key} twice.`);
    // "actor" records the caller's ID, so it cannot also name an input.
    if (key === SIGNAL_ACTOR && inputs.has(key)) {
      throw refusal("signal.property-actor-input", `Action ${name} names signal property actor, which records the caller's ID, and also has an input named actor. Rename the input.`);
    }
    if (key !== SIGNAL_ACTOR && !inputs.has(key)) throw refusal("signal.property-unbound", `Action ${name} names signal property ${key}, which is not one of its input bindings.`);
    seen.add(key);
  }
  return { name: record.name, properties: properties as string[] };
}

// The same checks, in the same order and with the same messages, as
// kernel-rs/kernel/src/http.rs `binding`. kernel-rs/kernel/src/http-binding-cases.json
// holds the cases both test suites run.
const HTTP_PATH = /^(\/[A-Za-z0-9._~-]+)+$/;
const RESERVED_SEGMENTS = new Set(["_konae", "oauth", ".well-known", "mcp"]);

function parseHttpBinding(name: string, kind: "query" | "mutation" | "signal", access: "anonymous" | "authenticated", value: unknown, bindings: ActionBinding[]): HttpBinding {
  if (kind === "signal") {
    throw refusal("signal.route", `Action ${name} is a signal action, so it takes no route. To record a signal from an HTTP route, declare the signal on a mutation with a POST route.`);
  }
  const invalid = `Action ${name} has an invalid http binding.`;
  if (!value || typeof value !== "object" || Array.isArray(value)) throw refusal("route.invalid", invalid);
  const record = value as Record<string, unknown>;
  if (Object.keys(record).some((key) => !["method", "path", "signature", "callers", "fields"].includes(key))) throw refusal("route.invalid", invalid);
  if (typeof record.path !== "string") throw refusal("route.invalid", invalid);
  if (!((record.method === "GET" && kind === "query") || (record.method === "POST" && kind === "mutation"))) {
    throw refusal("route.method-mismatch", `Action ${name} must bind GET for a query and POST for a mutation.`);
  }
  if (access !== "anonymous") {
    throw refusal("route.authenticated", `Action ${name} is authenticated, so it cannot declare http. Only anonymous actions answer on an HTTP route.`);
  }
  const path = record.path;
  const segments = path.split("/").slice(1);
  if (path.length > 200 || !HTTP_PATH.test(path) || segments.some((segment) => segment === "." || segment === "..")) {
    throw refusal("route.path-invalid", `Action ${name} has an invalid http path. Use a leading "/" and segments of letters, digits, ".", "_", "~" or "-", up to 200 characters.`);
  }
  if (RESERVED_SEGMENTS.has(segments[0].toLowerCase())) {
    throw refusal("route.path-reserved", `Action ${name} binds the reserved path ${path}. Paths under /_konae, /oauth, /.well-known and /mcp belong to the Kernel.`);
  }
  const binding: HttpBinding = { method: record.method, path };
  if (record.signature !== undefined) binding.signature = parseRouteSignature(name, record.method, record.signature);
  if (record.callers !== undefined) {
    const callers = record.callers;
    if (!Array.isArray(callers) || callers.length === 0 || new Set(callers).size !== callers.length
      || callers.some((caller) => !ROUTE_CALLERS.includes(caller as RouteCaller))) {
      throw refusal("route.callers-invalid", `Action ${name} has an invalid callers list. List one or more of "signature", "key" and "session", each once.`);
    }
    if (callers.includes("signature") && !binding.signature) {
      throw refusal("route.callers-no-signature", `Action ${name} lists "signature" among its callers but declares no signature.`);
    }
    if (binding.signature && !callers.includes("signature")) {
      throw refusal("route.callers-without-signature", `Action ${name} declares a signature, so its callers must include "signature".`);
    }
    binding.callers = callers as RouteCaller[];
  }
  if (record.fields !== undefined) {
    const inputs = new Set(bindings.flatMap((binding) => binding.source === "input" ? [binding.key] : []));
    binding.fields = parseRouteFields(name, record.method, inputs, record.fields);
  }
  return binding;
}

function assertDistinctRoutes(actions: Record<string, ServerActionDefinition>) {
  const seen = new Map<string, string>();
  for (const name of Object.keys(actions).sort()) {
    const http = actions[name].http;
    if (!http) continue;
    const route = `${http.method} ${http.path}`;
    const earlier = seen.get(route);
    if (earlier) throw refusal("route.duplicate", `Actions ${earlier} and ${name} both bind ${route}.`);
    seen.set(route, name);
  }
}

function parseBinding(actionName: string, index: number, value: unknown): ActionBinding {
  if (!value || typeof value !== "object") throw refusal("action.binding-invalid", `Invalid binding ${index} in ${actionName}.`);
  const binding = value as Record<string, unknown>;
  if (binding.nullable !== undefined && typeof binding.nullable !== "boolean" || binding.nullable === true && binding.source !== "input") {
    throw refusal("action.binding-nullable-invalid", `Action ${actionName} has an invalid nullable input binding.`);
  }
  if (binding.source === "input" && typeof binding.key === "string") {
    const nullable = typeof binding.nullable === "boolean" ? binding.nullable : undefined;
    if (binding.type === "string") {
      const maxLength = binding.maxLength === undefined ? 10_000 : Number(binding.maxLength);
      if (!Number.isInteger(maxLength) || maxLength < 1 || maxLength > 100_000) throw refusal("action.max-length-invalid", `Invalid maxLength in ${actionName}.`);
      return { source: "input", key: binding.key, type: "string", maxLength, ...(nullable === undefined ? {} : { nullable }) };
    }
    if (binding.type === "number" || binding.type === "boolean") return { source: "input", key: binding.key, type: binding.type, ...(nullable === undefined ? {} : { nullable }) };
  }
  if (binding.source === "actor" && binding.field === "id") return { source: "actor", field: "id" };
  if (binding.source === "kernel" && binding.field === "now") return { source: "kernel", field: "now" };
  // An absent value and null both bind NULL, as on the Kernel.
  if (binding.source === "literal" && (binding.value === undefined || binding.value === null || typeof binding.value === "string" || typeof binding.value === "number")) {
    return { source: "literal", value: binding.value ?? null };
  }
  throw refusal("action.binding-invalid", `Invalid binding ${index} in ${actionName}.`);
}

function resolveBinding(binding: ActionBinding, input: Record<string, unknown>, context: ResolveContext) {
  if (binding.source === "literal") return binding.value;
  if (binding.source === "kernel") return context.now ?? Date.now();
  if (binding.source === "actor") {
    if (!context.actorId) throw new Error("This action requires an authenticated actor.");
    return context.actorId;
  }
  const value = input[binding.key];
  if (binding.nullable && (value === undefined || value === null)) return null;
  // The Kernel's words, kernel-rs/kernel/src/lib.rs `resolve_bindings`, so the preview refuses as a Deployment does.
  if (value === undefined) throw new Error(`Missing action input: ${binding.key}`);
  if (binding.type === "string") {
    if (typeof value !== "string" || !value.trim()) throw new Error(`${binding.key} must be a non-empty string.`);
    if (value.length > (binding.maxLength ?? 10_000)) throw new Error(`${binding.key} is too long.`);
    return value;
  }
  if (binding.type === "number") {
    if (typeof value !== "number" || !Number.isFinite(value)) throw new Error(`${binding.key} must be a number.`);
    return value;
  }
  if (typeof value !== "boolean") throw new Error(`${binding.key} must be a boolean.`);
  return value ? 1 : 0;
}
