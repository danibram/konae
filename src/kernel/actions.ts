import type { RuntimeActionDeclaration } from "../deployments/manifest";
import { unboundedQueryReason } from "../lib/query-bound";
import { assertRuntimeSql, validateRuntimeParams } from "../runtime/security";
import { assertActionStatement } from "./sql-rules";

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
  /** `secret` names a value the owner sets with `secret_set`; it is never in actions.json. */
  signature?: { header: string; secret: string };
  /**
   * The route's callers. Without it a signed route takes the signature or a
   * key, an unsigned route on a private Deployment a key or a session, and an
   * unsigned route on a public Deployment anyone. See `routeCallers`.
   */
  callers?: RouteCaller[];
};

const ROUTE_CALLERS: readonly RouteCaller[] = ["signature", "key", "session"];

/**
 * The callers a route accepts on a Deployment of the given visibility, or
 * null when it answers anyone. An action whose route does not answer anyone
 * answers only there, never through actions_invoke (ADR-0011, update of
 * 2026-11-06). kernel-rs/src/http.rs `route_callers` is the same rule.
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
};

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

  const actions: Record<string, ServerActionDefinition> = {};
  for (const [name, value] of Object.entries(record.actions as Record<string, unknown>)) {
    actions[name] = parseAction(name, value);
  }
  assertDistinctRoutes(actions);
  if (Object.keys(actions).length > 50) throw new Error("A deployment may declare at most 50 server actions.");
  return { version: 1, actions };
}

export function actionDeclarations(program: ServerActionProgram): RuntimeActionDeclaration[] {
  return Object.values(program.actions).map(({ name, kind, access }) => ({ name, kind, access }));
}

export function resolveServerAction(
  program: ServerActionProgram,
  name: string,
  input: unknown,
  context: { actorId: string | null; now?: number },
) {
  const action = program.actions[name];
  if (!action) throw new Error(`Server action not declared: ${name}`);
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

function parseAction(name: string, value: unknown): ServerActionDefinition {
  if (!/^[a-z][a-z0-9]*(?:\.[a-z][a-z0-9]*)+$/.test(name)) throw new Error(`Invalid server action name: ${name}`);
  if (!value || typeof value !== "object") throw new Error(`Invalid server action: ${name}`);
  const candidate = value as Record<string, unknown>;
  const kind = candidate.kind === "query" || candidate.kind === "mutation" || candidate.kind === "signal" ? candidate.kind : null;
  if (candidate.access === "granted") {
    // Reserved for per-grant access; the Kernel refuses to deploy it until grants exist.
    throw new Error(`Action ${name} uses "access": "granted", which the Kernel does not support yet. Use "anonymous" or "authenticated".`);
  }
  const access = candidate.access === "anonymous" || candidate.access === "authenticated" ? candidate.access : null;
  if (kind === "signal" && access) {
    if (candidate.statement !== undefined && !(typeof candidate.statement === "string" && !candidate.statement.trim())) {
      throw new Error(`Action ${name} is a signal action, so it runs no SQL and cannot declare a statement.`);
    }
    if (!Array.isArray(candidate.bindings)) throw new Error(`Action ${name} requires a bindings array.`);
    const bindings = candidate.bindings.map((binding, index) => parseBinding(name, index, binding));
    const signal = parseSignal(name, kind, bindings, candidate.signal);
    const http = candidate.http === undefined ? undefined : parseHttpBinding(name, kind, access, candidate.http);
    parseGuard(name, kind, candidate.guard);
    return { name, kind, access, statement: "", bindings, ...(signal ? { signal } : {}), ...(http ? { http } : {}) };
  }
  if (!kind || !access || typeof candidate.statement !== "string") throw new Error(`Invalid server action: ${name}`);
  assertActionStatement(name, kind === "query" ? "query" : "mutation", candidate.statement);
  assertRuntimeSql(kind === "query" ? "query" : "execute", candidate.statement);
  if (kind === "query") {
    const reason = unboundedQueryReason(candidate.statement);
    if (reason) {
      throw new Error(`Action ${name} ${reason} Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.`);
    }
  }
  if (!Array.isArray(candidate.bindings)) throw new Error(`Action ${name} requires a bindings array.`);
  const bindings = candidate.bindings.map((binding, index) => parseBinding(name, index, binding));
  const signal = parseSignal(name, kind, bindings, candidate.signal);
  const http = candidate.http === undefined ? undefined : parseHttpBinding(name, kind, access, candidate.http);
  const guard = parseGuard(name, kind, candidate.guard);
  return {
    name, kind, access, statement: candidate.statement.trim(), bindings,
    ...(signal ? { signal } : {}), ...(http ? { http } : {}), ...(guard ? { guard } : {}),
  };
}

// The same checks and messages as kernel-rs/src/guard.rs `declaration`.
// kernel-rs/src/guard-cases.json holds the cases both test suites run.
const MAX_GUARD_LENGTH = 200;

function parseGuard(name: string, kind: "query" | "mutation" | "signal", value: unknown): string | undefined {
  if (value === undefined) return undefined;
  if (kind !== "mutation") throw new Error(`Action ${name} is a ${kind === "query" ? "query" : "signal action"}, so it cannot declare a guard. Declare it on a mutation.`);
  // Characters, not bytes or UTF-16 units, so both doors count an accented letter once.
  if (typeof value !== "string" || !value.trim() || [...value].length > MAX_GUARD_LENGTH || /[\u0000-\u001f\u007f]/.test(value)) {
    throw new Error(`Action ${name} has an invalid guard. Use a message of 1 to ${MAX_GUARD_LENGTH} characters on one line.`);
  }
  return value;
}

// The same checks, in the same order and with the same messages, as
// kernel-rs/src/usage.rs `declaration`. kernel-rs/src/signal-cases.json holds
// the cases both test suites run.
const SIGNAL_NAME = /^[a-z][a-z0-9]*(?:\.[a-z][a-z0-9]*)+$/;
const MAX_SIGNAL_PROPERTIES = 8;
/** The property that records the caller's ID: a user's, a key's, or null for a visitor or a signed request. */
const SIGNAL_ACTOR = "actor";

function parseSignal(name: string, kind: "query" | "mutation" | "signal", bindings: ActionBinding[], value: unknown): SignalDeclaration | undefined {
  if (value === undefined) {
    if (kind === "signal") throw new Error(`Action ${name} is a signal action, so it must declare a signal.`);
    return undefined;
  }
  if (kind === "query") {
    throw new Error(`Action ${name} is a query, so it cannot declare a signal. Declare it on a mutation or on a "kind": "signal" action.`);
  }
  const invalid = `Action ${name} has an invalid signal declaration.`;
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(invalid);
  const record = value as Record<string, unknown>;
  if (Object.keys(record).some((key) => key !== "name" && key !== "properties")) throw new Error(invalid);
  if (typeof record.name !== "string") throw new Error(invalid);
  if (record.name.length > 64 || !SIGNAL_NAME.test(record.name)) {
    throw new Error(`Action ${name} names an invalid signal. Use dot-separated lowercase words, such as "order.placed", up to 64 characters.`);
  }
  const properties = record.properties === undefined ? [] : record.properties;
  if (!Array.isArray(properties) || properties.some((key) => typeof key !== "string")) throw new Error(invalid);
  if (properties.length > MAX_SIGNAL_PROPERTIES) throw new Error(`Action ${name} names more than ${MAX_SIGNAL_PROPERTIES} signal properties.`);
  const inputs = new Set(bindings.flatMap((binding) => binding.source === "input" ? [binding.key] : []));
  const seen = new Set<string>();
  for (const key of properties as string[]) {
    if (seen.has(key)) throw new Error(`Action ${name} names signal property ${key} twice.`);
    // "actor" records the caller's ID, so it cannot also name an input.
    if (key === SIGNAL_ACTOR && inputs.has(key)) {
      throw new Error(`Action ${name} names signal property actor, which records the caller's ID, and also has an input named actor. Rename the input.`);
    }
    if (key !== SIGNAL_ACTOR && !inputs.has(key)) throw new Error(`Action ${name} names signal property ${key}, which is not one of its input bindings.`);
    seen.add(key);
  }
  return { name: record.name, properties: properties as string[] };
}

// The same checks, in the same order and with the same messages, as
// kernel-rs/src/http.rs `binding`. kernel-rs/src/http-binding-cases.json
// holds the cases both test suites run.
const HTTP_PATH = /^(\/[A-Za-z0-9._~-]+)+$/;
const RESERVED_SEGMENTS = new Set(["_littlebox", "oauth", ".well-known", "mcp"]);
const SIGNATURE_HEADER = /^[A-Za-z0-9-]{1,64}$/;
const UNSIGNABLE_HEADERS = new Set(["authorization", "cookie", "host", "content-type", "content-length"]);
const SECRET_NAME = /^[a-z][a-z0-9_]{0,63}$/;

function parseHttpBinding(name: string, kind: "query" | "mutation" | "signal", access: "anonymous" | "authenticated", value: unknown): HttpBinding {
  const invalid = `Action ${name} has an invalid http binding.`;
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(invalid);
  const record = value as Record<string, unknown>;
  if (Object.keys(record).some((key) => !["method", "path", "signature", "callers"].includes(key))) throw new Error(invalid);
  if (typeof record.path !== "string") throw new Error(invalid);
  if (!((record.method === "GET" && kind === "query") || (record.method === "POST" && kind === "mutation"))) {
    throw new Error(`Action ${name} must bind GET for a query and POST for a mutation.`);
  }
  if (access !== "anonymous") {
    throw new Error(`Action ${name} is authenticated, so it cannot declare http. Only anonymous actions answer on an HTTP route.`);
  }
  const path = record.path;
  const segments = path.split("/").slice(1);
  if (path.length > 200 || !HTTP_PATH.test(path) || segments.some((segment) => segment === "." || segment === "..")) {
    throw new Error(`Action ${name} has an invalid http path. Use a leading "/" and segments of letters, digits, ".", "_", "~" or "-", up to 200 characters.`);
  }
  if (RESERVED_SEGMENTS.has(segments[0].toLowerCase())) {
    throw new Error(`Action ${name} binds the reserved path ${path}. Paths under /_littlebox, /oauth, /.well-known and /mcp belong to the Kernel.`);
  }
  const binding: HttpBinding = { method: record.method, path };
  if (record.signature !== undefined) {
    const signature = record.signature as Record<string, unknown> | null;
    if (!signature || typeof signature !== "object" || Array.isArray(signature)
      || Object.keys(signature).some((key) => key !== "header" && key !== "secret")) throw new Error(invalid);
    if (typeof signature.header !== "string" || !SIGNATURE_HEADER.test(signature.header) || UNSIGNABLE_HEADERS.has(signature.header.toLowerCase())) {
      throw new Error(`Action ${name} has an invalid signature header.`);
    }
    if (typeof signature.secret !== "string" || !SECRET_NAME.test(signature.secret)) {
      throw new Error(`Action ${name} names an invalid signature secret. Use lowercase letters, digits and "_", starting with a letter.`);
    }
    binding.signature = { header: signature.header, secret: signature.secret };
  }
  if (record.callers !== undefined) {
    const callers = record.callers;
    if (!Array.isArray(callers) || callers.length === 0 || new Set(callers).size !== callers.length
      || callers.some((caller) => !ROUTE_CALLERS.includes(caller as RouteCaller))) {
      throw new Error(`Action ${name} has an invalid callers list. List one or more of "signature", "key" and "session", each once.`);
    }
    if (callers.includes("signature") && !binding.signature) {
      throw new Error(`Action ${name} lists "signature" among its callers but declares no signature.`);
    }
    if (binding.signature && !callers.includes("signature")) {
      throw new Error(`Action ${name} declares a signature, so its callers must include "signature".`);
    }
    binding.callers = callers as RouteCaller[];
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
    if (earlier) throw new Error(`Actions ${earlier} and ${name} both bind ${route}.`);
    seen.set(route, name);
  }
}

function parseBinding(actionName: string, index: number, value: unknown): ActionBinding {
  if (!value || typeof value !== "object") throw new Error(`Invalid binding ${index} in ${actionName}.`);
  const binding = value as Record<string, unknown>;
  if (binding.nullable !== undefined && (typeof binding.nullable !== "boolean" || binding.source !== "input")) {
    throw new Error(`Invalid nullable option in ${actionName}.`);
  }
  if (binding.source === "input" && typeof binding.key === "string") {
    const nullable = typeof binding.nullable === "boolean" ? binding.nullable : undefined;
    if (binding.type === "string") {
      const maxLength = binding.maxLength === undefined ? 10_000 : Number(binding.maxLength);
      if (!Number.isInteger(maxLength) || maxLength < 1 || maxLength > 100_000) throw new Error(`Invalid maxLength in ${actionName}.`);
      return { source: "input", key: binding.key, type: "string", maxLength, ...(nullable === undefined ? {} : { nullable }) };
    }
    if (binding.type === "number" || binding.type === "boolean") return { source: "input", key: binding.key, type: binding.type, ...(nullable === undefined ? {} : { nullable }) };
  }
  if (binding.source === "actor" && binding.field === "id") return { source: "actor", field: "id" };
  if (binding.source === "kernel" && binding.field === "now") return { source: "kernel", field: "now" };
  if (binding.source === "literal" && (binding.value === null || typeof binding.value === "string" || typeof binding.value === "number")) {
    return { source: "literal", value: binding.value };
  }
  throw new Error(`Invalid binding ${index} in ${actionName}.`);
}

function resolveBinding(binding: ActionBinding, input: Record<string, unknown>, context: { actorId: string | null; now?: number }) {
  if (binding.source === "literal") return binding.value;
  if (binding.source === "kernel") return context.now ?? Date.now();
  if (binding.source === "actor") {
    if (!context.actorId) throw new Error("This action requires an authenticated actor.");
    return context.actorId;
  }
  const value = input[binding.key];
  if (binding.nullable && (value === undefined || value === null)) return null;
  if (binding.type === "string") {
    if (typeof value !== "string" || !value.trim()) throw new Error(`${binding.key} must be a non-empty string.`);
    if (value.length > (binding.maxLength ?? 10_000)) throw new Error(`${binding.key} is too long.`);
    return value;
  }
  if (binding.type === "number") {
    if (typeof value !== "number" || !Number.isFinite(value)) throw new Error(`${binding.key} must be a finite number.`);
    return value;
  }
  if (typeof value !== "boolean") throw new Error(`${binding.key} must be a boolean.`);
  return value ? 1 : 0;
}
