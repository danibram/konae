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
import { routeSignature, type ActionContract, type ContractAction, type ContractColumn, type ContractError } from "./action-contract";
import type { RouteCaller } from "../kernel/actions";
import { bodyFromFields } from "../kernel/route-declaration";
import { signDelivery } from "./provider-signature";
import { signRoute } from "./route-signature";

export type RuntimeAnswer = { ok: true; result: unknown } | { ok: false; message: string };
export type RouteAnswer = { status: number; body: unknown };
export type RouteRequest = {
  method: "GET" | "HEAD" | "POST";
  path: string;
  /** GET and HEAD: the query string values. */
  query?: Record<string, string>;
  /** POST only: the raw body. */
  body?: string;
  headers?: Record<string, string>;
  /** Sends the app session as a cookie from the app's own origin. */
  session?: boolean;
};

export interface ConformanceTarget {
  /** Calls an action on the runtime endpoint, with the session when `as` is "session". */
  invoke(name: string, input: unknown, as: "session" | "visitor"): Promise<RuntimeAnswer>;
  /** Sends a request to the Deployment host, or is absent when routes are not tested. */
  route?(request: RouteRequest): Promise<RouteAnswer>;
  /** Whether `invoke(..., "session")` carries a signed-in person, and `route` can send their cookie. */
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
  /**
   * Input values by action that replace the generated ones in valid calls,
   * for a string the statement reads as more than text, such as JSON for
   * json_each. The contract types an input; it cannot say what SQL makes of it.
   */
  inputs?: Record<string, Record<string, unknown>>;
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
  const minimal = (action: ContractAction): Record<string, unknown> => ({
    ...Object.fromEntries(action.inputs.map((input) => [
      input.key,
      input.type === "string" ? unique(input.maxLength ?? 10_000) : input.type === "number" ? 1 : true,
    ])),
    ...options.inputs?.[action.name],
  });

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
  const expectRouteRefusal = async (action: ContractAction | null, check: string, message: string, request: RouteRequest, answered?: RouteAnswer) => {
    const name = action?.name ?? "routes";
    const errors = action ? [...action.errors, ...contract.errors] : contract.errors;
    const error = errors.find((candidate) => candidate.via !== "runtime" && matches(candidate.message, message));
    if (!error) {
      record(name, check, "fail", `The contract does not list "${message}" for this route.`);
      return;
    }
    const answer = answered ?? await route(request);
    const got = errorOf(answer.body);
    if (answer.status === error.status && got === message) record(name, check, "pass", `Answered ${answer.status} "${got}".`);
    else record(name, check, "fail", `Answered ${answer.status} ${got === undefined ? preview(answer.body) : `"${got}"`} where the contract says ${error.status} "${message}".`);
  };

  const guardChecked = new Set<string>();

  /**
   * Probes a guard: a guarded mutation that changes no row must answer its
   * guard, never { changes: 0 }. The first answer may already be the guard;
   * otherwise the same input is sent again, which changes no row for most
   * statements (an UPDATE with a state check, an INSERT ... WHERE NOT EXISTS).
   * When the second call changes a row too, conform cannot reach the guard.
   */
  async function checkGuard(action: ContractAction, result: unknown, message: string | undefined, again: () => Promise<{ result?: unknown; message?: string }>) {
    guardChecked.add(action.name);
    const check = "refuses with its guard when no row changes";
    const judge = (answer: { result?: unknown; message?: string }) => {
      if (answer.message === action.guard) { record(action.name, check, "pass", `Refused with "${answer.message}".`); return true; }
      if (answer.message !== undefined) { record(action.name, check, "fail", `Refused with "${answer.message}" where the contract says "${action.guard}".`); return true; }
      if (changesOf(answer.result) === 0) { record(action.name, check, "fail", `Answered ${preview(answer.result)} where the contract refuses with "${action.guard}".`); return true; }
      return false;
    };
    if (judge({ result, message })) return;
    if (!judge(await again())) record(action.name, check, "skip", "Both calls changed a row, so conform could not make the statement change nothing.");
  }

  // The name no action declares.
  await expectRefusal(null, "refuses an undeclared action", ["Server action not declared.", "Sign in to use Konae MCP."],
    () => invoke(UNDECLARED_ACTION, {}, target.hasSession ? "session" : "visitor"));

  for (const action of contract.actions) {
    const name = action.name;
    const valid = minimal(action);
    // A call's then mutation runs only after its call ends; the runtime endpoint refuses it (ADR-0019).
    if (action.thenOf) {
      await expectRefusal(action, "runs only after its call", [`Action ${name} runs only after the call ${action.thenOf} answers.`, "Sign in to use Konae MCP."],
        () => invoke(name, valid, target.hasSession ? "session" : "visitor"));
      continue;
    }
    // A scheduled mutation with no route runs only on the Kernel's timer (ADR-0017, update of 2026-11-08).
    if (action.schedule && !action.http) {
      await expectRefusal(action, "runs only on its schedule", [`Action ${name} runs only on its schedule.`, "Sign in to use Konae MCP."],
        () => invoke(name, valid, target.hasSession ? "session" : "visitor"));
      continue;
    }
    const routeOnly = action.http ? `Action ${name} answers only on its HTTP route, ${action.http.method} ${action.http.path}.` : null;
    if (action.http?.onlyDoor === "always") {
      // The runtime endpoint refuses the action; its bindings are probed on the route.
      // A visitor of a private Deployment is stopped before the action, with "Sign in".
      await expectRefusal(action, "answers only on its route", [routeOnly!, "Sign in to use Konae MCP."], () => invoke(name, valid, target.hasSession ? "session" : "visitor"));
      await checkRoute(action, valid);
      continue;
    }
    // A caller that gets past access and the actor binding.
    const caller: "session" | "visitor" | null = target.hasSession ? "session"
      : action.access === "anonymous" && !action.actorBound ? "visitor" : null;
    // On a private Deployment an unsigned route is the action's only door too.
    let runtimeClosed = false;

    // The answer for a valid call.
    if (!caller) {
      record(name, "answers a valid call", "skip", "Needs a signed-in person; pass --token.");
    } else if (action.call) {
      // A valid call would queue a real request to another app or service.
      record(name, "answers a valid call", "skip", `A valid call queues ${action.call.method} ${action.call.url}, so conform does not send one.`);
    } else {
      const answer = await invoke(name, valid, caller);
      if (!answer.ok && answer.message === routeOnly && listedIn(action.errors, answer.message)) {
        runtimeClosed = true;
        record(name, "answers only on its route on this private Deployment", "pass", `Refused with "${answer.message}".`);
      } else if (!answer.ok && action.guard && answer.message === action.guard) {
        // The generated input matched no row, and the app's rule refused it as the contract says.
        record(name, "answers a valid call", "pass", "Refused with its guard: the generated input changed no row.");
        await checkGuard(action, undefined, answer.message, async () => ({}));
      } else if (!answer.ok) {
        const advice = answer.message === "Sign in to use Konae MCP." ? " The Deployment is private; pass --token."
          : listedIn([...action.errors, ...contract.errors], answer.message) ? ""
          : " The contract lists no such refusal, so the statement itself failed. If an input needs a particular form, such as JSON, give one with --inputs.";
        record(name, "answers a valid call", "fail", `Refused with "${answer.message}".${advice}`);
      } else {
        const problem = checkOutput(action, answer.result);
        record(name, "answers a valid call", problem ? "fail" : "pass", problem ?? describeOutput(action, answer.result));
        if (action.guard && !problem) {
          await checkGuard(action, answer.result, undefined, () => invoke(name, valid, caller).then((again) => again.ok ? { result: again.result } : { message: again.message }));
        }
      }
      for (const input of runtimeClosed ? [] : action.inputs.filter((candidate) => candidate.nullable)) {
        const { [input.key]: _, ...rest } = valid;
        const omitted = await invoke(name, rest, caller);
        if (omitted.ok) record(name, `answers without the nullable ${input.key}`, checkOutput(action, omitted.result) ? "fail" : "pass", checkOutput(action, omitted.result) ?? "Answered.");
        // The bindings accepted the call and the app's rule refused it, as for a caller without the role.
        else if (action.guard && omitted.message === action.guard) record(name, `answers without the nullable ${input.key}`, "pass", `Refused with its guard, "${omitted.message}": the bindings accepted the call without ${input.key}.`);
        else record(name, `answers without the nullable ${input.key}`, "fail", `Refused with "${omitted.message}" where the contract makes ${input.key} optional.`);
      }
    }

    // Who may call.
    if (action.access === "authenticated") {
      await expectRefusal(action, "refuses a caller without a session", ["authentication-required", "Sign in to use Konae MCP."], () => invoke(name, valid, "visitor"));
    } else if (action.actorBound) {
      await expectRefusal(action, "refuses a caller without an ID", ["This action requires an authenticated actor.", "Sign in to use Konae MCP."], () => invoke(name, valid, "visitor"));
    }

    // What the bindings refuse.
    if (runtimeClosed) {
      // Probed on the route below.
    } else if (!caller) {
      record(name, "refuses bad input", "skip", "Needs a signed-in person; pass --token.");
    } else {
      await expectRefusal(action, "refuses an undeclared input", [`Unexpected action input: ${UNEXPECTED_KEY}`], () => invoke(name, { ...valid, [UNEXPECTED_KEY]: 1 }, caller));
      for (const input of action.inputs) {
        if (input.required) {
          const { [input.key]: _, ...rest } = valid;
          await expectRefusal(action, `refuses a missing ${input.key}`, [`Missing action input: ${input.key}`], () => invoke(name, rest, caller));
        }
        if (input.type === "string") {
          await expectRefusal(action, `refuses ${input.key} when it is not a string`, [`${input.key} must be a non-empty string.`], () => invoke(name, { ...valid, [input.key]: 5 }, caller));
          await expectRefusal(action, `refuses an empty ${input.key}`, [`${input.key} must be a non-empty string.`], () => invoke(name, { ...valid, [input.key]: " " }, caller));
          const maxLength = input.maxLength ?? 10_000;
          // The runtime endpoint reads at most 200 KB per request.
          if (maxLength < 150_000) {
            await expectRefusal(action, `refuses ${input.key} when it is longer than ${maxLength}`, [`${input.key} is too long.`], () => invoke(name, { ...valid, [input.key]: "x".repeat(maxLength + 1) }, caller));
          }
        } else if (input.type === "number") {
          await expectRefusal(action, `refuses ${input.key} when it is not a number`, [`${input.key} must be a number.`], () => invoke(name, { ...valid, [input.key]: "1" }, caller));
        } else {
          await expectRefusal(action, `refuses ${input.key} when it is not a boolean`, [`${input.key} must be a boolean.`], () => invoke(name, { ...valid, [input.key]: "true" }, caller));
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
    // Without declared callers, a signed route takes the signature or a key and an unsigned one a key or a session.
    const takes = (caller: RouteCaller) => http.declaredCallers ? http.declaredCallers.includes(caller) : caller !== "session" || !http.signature;
    // With fields, each input sits at its path, and a key no input names rides along to show the route leaves it out.
    const postBody = (input: Record<string, unknown>) => JSON.stringify(http.fields ? { ...bodyFromFields(http.fields, input), [UNEXPECTED_KEY]: 1 } : input);
    const request = (input: Record<string, unknown>, body?: string): RouteRequest => http.method === "GET"
      ? { method: "GET", path: http.path, query: Object.fromEntries(Object.entries(input).map(([key, value]) => [key, String(value)])) }
      : { method: "POST", path: http.path, body: body ?? postBody(input), headers: { "content-type": "application/json" } };
    // A body-only MAC is the same for the same bytes, so each signed request gets its own trailing whitespace, which JSON ignores.
    let resends = 0;
    const sign = async (base: RouteRequest, at = now()): Promise<RouteRequest> => {
      const signature = http.signature!;
      if (signature.scheme === "konae") {
        const query = new URLSearchParams(base.query ?? {}).toString();
        const header = await signRoute(secret!, { method: http.method, path: base.path, query, body: base.body ?? "" }, at);
        return { ...base, headers: { ...base.headers, [signature.header]: header } };
      }
      const body = signature.scheme === "hmac-body" ? `${base.body ?? ""}${" ".repeat(resends++)}` : base.body ?? "";
      return { ...base, body, headers: { ...base.headers, ...await signDelivery(routeSignature(signature), secret!, body, { t: at }) } };
    };
    // The proof the valid call and the input probes carry. An action that binds
    // the caller needs one with an ID: a key or a session.
    type Proof = "signature" | "key" | "session" | "none";
    // An unsigned route without callers answers anyone on a public Deployment,
    // so it is tried with no proof first; a key or a session follows on a private one.
    const usable = (candidate: Proof) => candidate === "signature" ? Boolean(http.signature && secret !== undefined)
      : candidate === "key" ? takes("key") && Boolean(target.key)
      : candidate === "session" ? takes("session") && target.hasSession
      : !http.signature && !http.declaredCallers;
    const proofs: Proof[] = action.actorBound ? ["key", "session"] : ["signature", "none", "key", "session"];
    let proof = proofs.find(usable);
    const fallback = (["key", "session"] as Proof[]).find(usable);
    const prove = async (base: RouteRequest): Promise<RouteRequest> => proof === "signature" ? sign(base)
      : proof === "key" ? { ...base, headers: { ...base.headers, authorization: `Bearer ${target.key}` } }
      : proof === "session" ? { ...base, session: true }
      : base;
    let open = Boolean(proof);

    if (!proof) {
      const needs = action.actorBound ? "an API key or a session (--key or --token)"
        : [http.signature ? `the ${http.signature.secret} secret (--secret ${http.signature.secret}=<value>)` : null, takes("key") ? "an API key (--key)" : null, takes("session") ? "a session (--token)" : null].filter(Boolean).join(" or ");
      record(name, `answers ${http.method} ${http.path}`, "skip", `Needs ${needs}.`);
    } else {
      let sent = await prove(request(valid));
      let answer = await route(sent);
      if (proof === "none" && answer.status === 401 && errorOf(answer.body) === "authentication-required") {
        // A private Deployment: the unsigned route takes a key or a session.
        await expectRouteRefusal(action, "refuses a caller without a key or a session", "authentication-required", sent, answer);
        proof = fallback;
        if (proof) {
          sent = await prove(request(valid));
          answer = await route(sent);
        }
      }
      const via = proof === "key" ? " with an API key" : proof === "session" ? " with a session" : "";
      const result = answer.body && typeof answer.body === "object" && "result" in answer.body ? (answer.body as { result: unknown }).result : undefined;
      if (!proof) {
        open = false;
        record(name, `answers ${http.method} ${http.path}`, "skip", "The Deployment is private, so the route takes an API key or a session; pass --key or --token.");
      } else if (action.guard && answer.status === 403 && errorOf(answer.body) === action.guard) {
        record(name, `answers ${http.method} ${http.path}${via}`, "pass", "Refused 403 with its guard: the generated input changed no row.");
        if (!guardChecked.has(name)) await checkGuard(action, undefined, action.guard, async () => ({}));
      } else {
        const problem = answer.status !== 200 ? `Answered ${answer.status} ${preview(answer.body)}.` : result === undefined ? `Answered ${preview(answer.body)} without "result".` : checkOutput(action, result);
        record(name, `answers ${http.method} ${http.path}${via}`, problem ? "fail" : "pass", problem ?? describeOutput(action, result));
        // The same signed request a second time.
        if (proof === "signature" && answer.status === 200) await expectRouteRefusal(action, "refuses a replayed signature", "replayed-signature", sent);
        if (action.guard && !problem && !guardChecked.has(name)) {
          await checkGuard(action, result, undefined, async () => {
            const again = await route(await prove(request(valid)));
            const message = errorOf(again.body);
            if (again.status === 403 && message !== undefined) return { message };
            return again.status === 200 && again.body && typeof again.body === "object" && "result" in again.body
              ? { result: (again.body as { result: unknown }).result } : { message: message ?? `HTTP ${again.status}` };
          });
        }
      }
    }

    if (http.signature) {
      await expectRouteRefusal(action, "refuses an unsigned request", "invalid-signature", request(valid));
      const window = http.signature.windowSeconds;
      if (secret !== undefined && window !== null) await expectRouteRefusal(action, "refuses a stale timestamp", "stale-timestamp", await sign(request(valid), now() - window - 100));
    } else if (http.declaredCallers) {
      await expectRouteRefusal(action, "refuses a caller without a key or a session", "authentication-required", request(valid));
    }
    if (http.method === "GET") {
      // HTTP carries no body on a HEAD answer, so only the status can be checked.
      const head = "A route does not answer HEAD. Send GET.";
      const error = action.errors.find((candidate) => candidate.via !== "runtime" && candidate.message === head);
      if (!error) record(name, "refuses HEAD", "fail", `The contract does not list "${head}" for this route.`);
      else {
        const answer = await route({ method: "HEAD", path: http.path, query: request(valid).query });
        record(name, "refuses HEAD", answer.status === error.status ? "pass" : "fail", answer.status === error.status
          ? `Answered ${answer.status}; a HEAD answer carries no body.` : `Answered ${answer.status} where the contract says ${error.status}.`);
      }
    }
    await expectRouteRefusal(action, "refuses an unknown API key", "invalid-api-key", { ...request(valid), headers: { ...request(valid).headers, authorization: "Bearer lbk_conformance_unknown" } });
    if (!takes("key") && target.key) {
      await expectRouteRefusal(action, "refuses an API key it does not take", "This route does not accept API keys.", { ...request(valid), headers: { ...request(valid).headers, authorization: `Bearer ${target.key}` } });
    }
    if (open) {
      // A route with fields leaves the key out instead; the valid call above carried one.
      if (!http.fields) await expectRouteRefusal(action, "refuses an undeclared input on the route", `Unexpected action input: ${UNEXPECTED_KEY}`, await prove(request({ ...valid, [UNEXPECTED_KEY]: 1 })));
      if (http.method === "POST") {
        await expectRouteRefusal(action, "refuses a body that is not JSON", "The request body must be a JSON object.", await prove(request(valid, "not json")));
      }
    }
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
  if (output.kind === "queued") {
    const record = result as Record<string, unknown> | null;
    return record && typeof record === "object" && typeof record.callId === "string" && record.queued === true ? null : `Answered ${preview(result)} where the contract says { callId, queued: true }.`;
  }
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

function changesOf(result: unknown): number | undefined {
  return result && typeof result === "object" && typeof (result as { changes?: unknown }).changes === "number" ? (result as { changes: number }).changes : undefined;
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
