/**
 * The conformance runner: calls every action an Action Contract lists against
 * a backend and checks that it answers with the shapes the contract states and
 * refuses what the contract says it refuses, with the same message.
 *
 * It talks to the backend through a ConformanceTarget, so the same checks run
 * against a Kernel over MCP and HTTP (scripts/conform.ts) and against a fake in
 * the tests. A replacement backend passes when every check passes; a check the
 * caller gave no credential for is reported as skipped, never as passed.
 */
import type { ActionContract, ContractAction, ContractColumn, ContractError } from "./action-contract";

export type RuntimeAnswer = { ok: true; result: unknown } | { ok: false; message: string };
export type RouteAnswer = { status: number; body: unknown };
export type RouteRequest = {
  method: "GET" | "POST";
  path: string;
  /** GET only: the query string values. */
  query?: Record<string, string>;
  /** POST only: the raw body. */
  body?: string;
  headers?: Record<string, string>;
};

export interface ConformanceTarget {
  /** Calls an action on the runtime endpoint, with the session when `as` is "session". */
  invoke(name: string, input: unknown, as: "session" | "visitor"): Promise<RuntimeAnswer>;
  /** Sends a request to the Deployment host, or is absent when routes are not tested. */
  route?(request: RouteRequest): Promise<RouteAnswer>;
  /** Whether `invoke(..., "session")` carries a signed-in person. */
  hasSession: boolean;
  /** An API key the backend issued, sent as Authorization: Bearer. */
  key?: string;
  /** Signing secret values by name, for signed routes. */
  secrets?: Record<string, string>;
}

export type CheckResult = {
  action: string;
  check: string;
  outcome: "pass" | "fail" | "skip";
  detail: string;
};

export type ConformanceReport = {
  results: CheckResult[];
  passed: number;
  failed: number;
  skipped: number;
  /** How many times a call waited out a rate limit and was sent again. */
  rateLimitWaits: number;
};

export type ConformanceOptions = {
  /** Makes generated strings unique per run, so a UNIQUE column does not refuse a second run. */
  nonce?: string;
  /** Waits before a rate-limited call is sent again. Tests pass a no-op. */
  wait?: (ms: number) => Promise<void>;
  /** Seconds since the epoch, for signatures. */
  now?: () => number;
};

const UNEXPECTED_KEY = "conformProbe";
const UNDECLARED_ACTION = "conform.undeclared";

/** Runs every check of the contract against the target, one call at a time. */
export async function runConformance(contract: ActionContract, target: ConformanceTarget, options: ConformanceOptions = {}): Promise<ConformanceReport> {
  const nonce = options.nonce ?? Math.random().toString(36).slice(2, 10);
  const wait = options.wait ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  const now = options.now ?? (() => Math.floor(Date.now() / 1000));
  const results: CheckResult[] = [];
  let rateLimitWaits = 0;

  const record = (action: string, check: string, outcome: CheckResult["outcome"], detail: string) => {
    results.push({ action, check, outcome, detail });
  };

  /** A runtime call that waits out `rate-limited` up to three times. */
  const invoke = async (name: string, input: unknown, as: "session" | "visitor"): Promise<RuntimeAnswer> => {
    for (let attempt = 0; ; attempt += 1) {
      const answer = await target.invoke(name, input, as);
      if (answer.ok || answer.message !== "rate-limited" || attempt === 3) return answer;
      rateLimitWaits += 1;
      await wait(10_000);
    }
  };
  const route = async (request: RouteRequest): Promise<RouteAnswer> => {
    for (let attempt = 0; ; attempt += 1) {
      const answer = await target.route!(request);
      if (answer.status !== 429 || attempt === 3) return answer;
      rateLimitWaits += 1;
      await wait(10_000);
    }
  };

  const unique = (maxLength: number) => `c${nonce}`.slice(0, maxLength);
  const minimal = (action: ContractAction): Record<string, unknown> => Object.fromEntries(action.inputs.map((input) => [
    input.key,
    input.type === "string" ? unique(input.maxLength ?? 10_000) : input.type === "number" ? 1 : true,
  ]));

  /** Checks a runtime refusal: the contract lists the message, and the backend answers with it. */
  const expectRefusal = async (action: ContractAction | null, check: string, messages: string[], call: () => Promise<RuntimeAnswer>) => {
    const name = action?.name ?? UNDECLARED_ACTION;
    const listed = messages.filter((message) => listedIn(action ? [...action.errors, ...contract.errors] : contract.errors, message));
    if (listed.length === 0) {
      record(name, check, "fail", `The contract does not list ${quoteAll(messages)} for this action.`);
      return;
    }
    const answer = await call();
    if (answer.ok) record(name, check, "fail", `Answered ${preview(answer.result)} where the contract refuses with ${quoteAll(listed)}.`);
    else if (listed.includes(answer.message)) record(name, check, "pass", `Refused with "${answer.message}".`);
    else record(name, check, "fail", `Refused with "${answer.message}" where the contract says ${quoteAll(listed)}.`);
  };

  /** Checks a route refusal by status and message. */
  const expectRouteRefusal = async (action: ContractAction | null, check: string, message: string, request: RouteRequest) => {
    const name = action?.name ?? "routes";
    const errors = action ? [...action.errors, ...contract.errors] : contract.errors;
    const error = errors.find((candidate) => candidate.via !== "runtime" && matches(candidate.message, message));
    if (!error) {
      record(name, check, "fail", `The contract does not list "${message}" for this route.`);
      return;
    }
    const answer = await route(request);
    const got = errorOf(answer.body);
    if (answer.status === error.status && got === message) record(name, check, "pass", `Answered ${answer.status} "${got}".`);
    else record(name, check, "fail", `Answered ${answer.status} ${got === undefined ? preview(answer.body) : `"${got}"`} where the contract says ${error.status} "${message}".`);
  };

  // The name no action declares.
  await expectRefusal(null, "refuses an undeclared action", ["Server action not declared.", "Sign in to use Littlebox MCP."],
    () => invoke(UNDECLARED_ACTION, {}, target.hasSession ? "session" : "visitor"));

  for (const action of contract.actions) {
    const name = action.name;
    const valid = minimal(action);
    // A caller that gets past access and the actor binding.
    const caller: "session" | "visitor" | null = target.hasSession ? "session"
      : action.access === "anonymous" && !action.actorBound ? "visitor" : null;

    // The answer for a valid call.
    if (!caller) {
      record(name, "answers a valid call", "skip", "Needs a signed-in person; pass --token.");
    } else {
      const answer = await invoke(name, valid, caller);
      if (!answer.ok) {
        const advice = answer.message === "Sign in to use Littlebox MCP." ? " The Deployment is private; pass --token." : "";
        record(name, "answers a valid call", "fail", `Refused with "${answer.message}".${advice}`);
      } else {
        const problem = checkOutput(action, answer.result);
        record(name, "answers a valid call", problem ? "fail" : "pass", problem ?? describeOutput(action, answer.result));
      }
      for (const input of action.inputs.filter((candidate) => candidate.nullable)) {
        const { [input.key]: _, ...rest } = valid;
        const omitted = await invoke(name, rest, caller);
        if (omitted.ok) record(name, `answers without the nullable ${input.key}`, checkOutput(action, omitted.result) ? "fail" : "pass", checkOutput(action, omitted.result) ?? "Answered.");
        else record(name, `answers without the nullable ${input.key}`, "fail", `Refused with "${omitted.message}" where the contract makes ${input.key} optional.`);
      }
    }

    // Who may call.
    if (action.access === "authenticated") {
      await expectRefusal(action, "refuses a caller without a session", ["authentication-required", "Sign in to use Littlebox MCP."], () => invoke(name, valid, "visitor"));
    } else if (action.actorBound) {
      await expectRefusal(action, "refuses a caller without an ID", ["This action requires an authenticated actor.", "Sign in to use Littlebox MCP."], () => invoke(name, valid, "visitor"));
    }

    // What the bindings refuse.
    if (!caller) {
      record(name, "refuses bad input", "skip", "Needs a signed-in person; pass --token.");
    } else {
      await expectRefusal(action, "refuses an undeclared input", [`Unexpected action input: ${UNEXPECTED_KEY}`], () => invoke(name, { ...valid, [UNEXPECTED_KEY]: 1 }, caller));
      for (const input of action.inputs) {
        if (input.required) {
          const { [input.key]: _, ...rest } = valid;
          await expectRefusal(action, `refuses a missing ${input.key}`, [`Missing action input: ${input.key}`], () => invoke(name, rest, caller));
        }
        if (input.type === "string") {
          await expectRefusal(action, `refuses a ${input.key} that is not a string`, [`${input.key} must be a non-empty string.`], () => invoke(name, { ...valid, [input.key]: 5 }, caller));
          await expectRefusal(action, `refuses an empty ${input.key}`, [`${input.key} must be a non-empty string.`], () => invoke(name, { ...valid, [input.key]: " " }, caller));
          const maxLength = input.maxLength ?? 10_000;
          // The runtime endpoint reads at most 200 KB per request.
          if (maxLength < 150_000) {
            await expectRefusal(action, `refuses a ${input.key} longer than ${maxLength}`, [`${input.key} is too long.`], () => invoke(name, { ...valid, [input.key]: "x".repeat(maxLength + 1) }, caller));
          }
        } else if (input.type === "number") {
          await expectRefusal(action, `refuses a ${input.key} that is not a number`, [`${input.key} must be a number.`], () => invoke(name, { ...valid, [input.key]: "1" }, caller));
        } else {
          await expectRefusal(action, `refuses a ${input.key} that is not a boolean`, [`${input.key} must be a boolean.`], () => invoke(name, { ...valid, [input.key]: "true" }, caller));
        }
      }
    }

    if (action.http) await checkRoute(action, valid);
  }

  async function checkRoute(action: ContractAction, valid: Record<string, unknown>) {
    const http = action.http!;
    const name = action.name;
    if (!target.route) {
      record(name, `answers ${http.method} ${http.path}`, "skip", "Routes are not tested against this target.");
      return;
    }
    const secret = http.signature ? target.secrets?.[http.signature.secret] : undefined;
    const request = (input: Record<string, unknown>, body?: string): RouteRequest => http.method === "GET"
      ? { method: "GET", path: http.path, query: Object.fromEntries(Object.entries(input).map(([key, value]) => [key, String(value)])) }
      : { method: "POST", path: http.path, body: body ?? JSON.stringify(input), headers: { "content-type": "application/json" } };
    const signed = async (base: RouteRequest): Promise<RouteRequest> => {
      if (!http.signature || secret === undefined) return base;
      const t = now();
      return { ...base, headers: { ...base.headers, [http.signature.header]: `t=${t},v1=${await hmac(secret, `${t}.${base.body ?? ""}`)}` } };
    };
    const routeCaller = !action.actorBound;

    if (http.signature && secret === undefined && !target.key) {
      record(name, `answers ${http.method} ${http.path}`, "skip", `Needs the ${http.signature.secret} secret (--secret ${http.signature.secret}=<value>) or an API key.`);
    } else if (!routeCaller) {
      record(name, `answers ${http.method} ${http.path}`, "skip", "Binds the caller's ID; a route caller with an ID needs an API key or a session cookie.");
    } else {
      const base = request(valid);
      const viaKey = http.signature && secret === undefined;
      const sent = viaKey ? { ...base, headers: { ...base.headers, authorization: `Bearer ${target.key}` } } : await signed(base);
      const answer = await route(sent);
      const result = answer.body && typeof answer.body === "object" && "result" in answer.body ? (answer.body as { result: unknown }).result : undefined;
      const problem = answer.status !== 200 ? `Answered ${answer.status} ${preview(answer.body)}.` : result === undefined ? `Answered ${preview(answer.body)} without "result".` : checkOutput(action, result);
      record(name, `answers ${http.method} ${http.path}${viaKey ? " with an API key" : ""}`, problem ? "fail" : "pass", problem ?? describeOutput(action, result));
      // The same signed request a second time.
      if (http.signature && !viaKey && answer.status === 200) await expectRouteRefusal(action, "refuses a replayed signature", "replayed-signature", sent);
    }

    if (http.signature) {
      await expectRouteRefusal(action, "refuses an unsigned request", "invalid-signature", request(valid));
    }
    await expectRouteRefusal(action, "refuses an unknown API key", "invalid-api-key", { ...request(valid), headers: { ...request(valid).headers, authorization: "Bearer lbk_conformance_unknown" } });
    if (!http.signature || secret !== undefined) {
      await expectRouteRefusal(action, "refuses an undeclared input on the route", `Unexpected action input: ${UNEXPECTED_KEY}`, await signed(request({ ...valid, [UNEXPECTED_KEY]: 1 })));
      if (http.method === "POST") {
        await expectRouteRefusal(action, "refuses a body that is not JSON", "The request body must be a JSON object.", await signed(request(valid, "not json")));
      }
    }
  }

  async function hmac(secret: string, message: string): Promise<string> {
    const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(message)));
    return Array.from(mac, (byte) => byte.toString(16).padStart(2, "0")).join("");
  }

  // Every POST that names no route.
  if (target.route && contract.actions.some((action) => action.http)) {
    await expectRouteRefusal(null, "refuses a POST to a path no action binds", "No route accepts POST on this path.", { method: "POST", path: "/conform-unbound", body: "{}", headers: { "content-type": "application/json" } });
  }

  return {
    results,
    passed: results.filter((result) => result.outcome === "pass").length,
    failed: results.filter((result) => result.outcome === "fail").length,
    skipped: results.filter((result) => result.outcome === "skip").length,
    rateLimitWaits,
  };
}

/** Why `result` does not have the contract's output shape, or null when it does. */
export function checkOutput(action: ContractAction, result: unknown): string | null {
  const output = action.output;
  if (output.kind === "changes") {
    if (!result || typeof result !== "object" || Array.isArray(result)) return `Answered ${preview(result)} where the contract says { changes, lastInsertId }.`;
    const keys = Object.keys(result).sort();
    if (keys.join(",") !== "changes,lastInsertId") return `Answered the keys ${keys.join(", ") || "none"} where the contract says changes, lastInsertId.`;
    const record = result as Record<string, unknown>;
    if (!Number.isInteger(record.changes) || !Number.isInteger(record.lastInsertId)) return `Answered ${preview(result)}; changes and lastInsertId must be integers.`;
    return null;
  }
  if (output.kind === "signal") return result === null || typeof result === "object" ? null : `Answered ${preview(result)} for a Signal.`;
  if (!Array.isArray(result)) return `Answered ${preview(result)} where the contract says an array of rows.`;
  const named = output.columns.map((column) => column.name);
  for (const [index, row] of result.entries()) {
    if (!row || typeof row !== "object" || Array.isArray(row)) return `Row ${index} is ${preview(row)}, not an object.`;
    const keys = Object.keys(row);
    const missing = named.filter((name) => !keys.includes(name));
    if (missing.length > 0) return `Row ${index} has no ${missing.join(", ")}.`;
    const extra = keys.filter((key) => !named.includes(key));
    if (extra.length !== output.unnamed.length) return `Row ${index} has the keys ${keys.join(", ")} where the contract says ${[...named, ...output.unnamed.map((column) => `(${column.expression})`)].join(", ")}.`;
    for (const column of output.columns) {
      const problem = checkValue(column, (row as Record<string, unknown>)[column.name]);
      if (problem) return `Row ${index}: ${problem}`;
    }
  }
  return null;
}

function checkValue(column: ContractColumn, value: unknown): string | null {
  if (value === null) return column.nullable === false ? `${column.name} is null where the contract says it is not nullable.` : null;
  const ok = column.type === "integer" ? Number.isInteger(value)
    : column.type === "real" ? typeof value === "number"
    : column.type === "text" ? typeof value === "string"
    : column.type === "numeric" ? typeof value === "number" || typeof value === "string"
    : true;
  return ok ? null : `${column.name} is ${preview(value)} where the contract says ${column.type}.`;
}

function describeOutput(action: ContractAction, result: unknown): string {
  if (action.output.kind === "rows") {
    const rows = Array.isArray(result) ? result.length : 0;
    return rows === 0 ? "Answered an empty array; no row to check the columns against." : `Answered ${rows} row${rows === 1 ? "" : "s"} with the contract's columns.`;
  }
  return `Answered ${preview(result)}.`;
}

/** Whether `message` is one of `errors`, reading <key> and <name> as any text. */
function listedIn(errors: ContractError[], message: string): boolean {
  return errors.some((error) => matches(error.message, message));
}

function matches(template: string, message: string): boolean {
  if (template === message) return true;
  if (!/<(key|name)>/.test(template)) return false;
  const pattern = template.split(/<(?:key|name)>/).map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join(".+");
  return new RegExp(`^${pattern}$`).test(message);
}

function errorOf(body: unknown): string | undefined {
  return body && typeof body === "object" && typeof (body as { error?: unknown }).error === "string" ? (body as { error: string }).error : undefined;
}

function quoteAll(messages: string[]): string {
  return messages.map((message) => `"${message}"`).join(" or ");
}

function preview(value: unknown): string {
  const text = JSON.stringify(value);
  return text === undefined ? String(value) : text.length > 120 ? `${text.slice(0, 117)}...` : text;
}

/** One line per check, then the totals. */
export function formatConformanceReport(report: ConformanceReport): string {
  const lines = report.results.map((result) => `${result.outcome.toUpperCase().padEnd(4)} ${result.action}: ${result.check}. ${result.detail}`);
  lines.push(`${report.passed} passed, ${report.failed} failed, ${report.skipped} skipped${report.rateLimitWaits ? `, ${report.rateLimitWaits} rate-limit waits` : ""}.`);
  return lines.join("\n");
}
