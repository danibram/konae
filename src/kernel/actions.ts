import type { RuntimeActionDeclaration } from "../deployments/manifest";
import { unboundedQueryReason } from "../lib/query-bound";
import { assertRuntimeSql, validateRuntimeParams } from "../runtime/security";
import { sqlTokens } from "../lib/sql-tokens";

type ActionBinding =
  | { source: "input"; key: string; type: "string"; maxLength?: number; nullable?: boolean }
  | { source: "input"; key: string; type: "number" | "boolean"; nullable?: boolean }
  | { source: "actor"; field: "id" }
  | { source: "kernel"; field: "now" }
  | { source: "literal"; value: string | number | null };

export type ServerActionDefinition = RuntimeActionDeclaration & {
  statement: string;
  bindings: ActionBinding[];
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
  const kind = candidate.kind === "query" || candidate.kind === "mutation" ? candidate.kind : null;
  if (candidate.access === "granted") {
    // Reserved for per-grant access; the Kernel refuses to deploy it until grants exist.
    throw new Error(`Action ${name} uses "access": "granted", which the Kernel does not support yet. Use "anonymous" or "authenticated".`);
  }
  const access = candidate.access === "anonymous" || candidate.access === "authenticated" ? candidate.access : null;
  if (!kind || !access || typeof candidate.statement !== "string") throw new Error(`Invalid server action: ${name}`);
  assertRuntimeSql(kind === "query" ? "query" : "execute", candidate.statement);
  assertSingleStatement(candidate.statement);
  const tokens = sqlTokens(candidate.statement);
  if (kind === "mutation" && tokens.some((token, index) => {
    const previous = tokens[index - 1];
    const parameter = previous?.kind === "symbol" && [":", "@", "$"].includes(previous.text);
    return token.kind === "word" && token.depth === 0 && token.text.toUpperCase() === "RETURNING" && !parameter;
  })) {
    throw new Error(`Action ${name} cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.`);
  }
  if (kind === "query") {
    const reason = unboundedQueryReason(candidate.statement);
    if (reason) {
      throw new Error(`Action ${name} ${reason} Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.`);
    }
  }
  if (!Array.isArray(candidate.bindings)) throw new Error(`Action ${name} requires a bindings array.`);
  const bindings = candidate.bindings.map((binding, index) => parseBinding(name, index, binding));
  return { name, kind, access, statement: candidate.statement.trim(), bindings };
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

function assertSingleStatement(sql: string) {
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
      quote = char === "[" ? "]" : char;
      continue;
    }
    if (quote && char === quote) {
      if (next === quote && quote !== "]") { index += 1; continue; }
      quote = null;
      continue;
    }
    if (!quote && char === ";") { terminated = true; continue; }
    if (terminated && !/\s/.test(char)) throw new Error("Server actions must contain exactly one SQL statement.");
  }
  if (quote) throw new Error("Unterminated SQL string.");
}
