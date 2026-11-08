import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { generateActionContract, type ActionContract, type ContractAction } from "./action-contract";
import { checkOutput, formatConformanceReport, runConformance, type ConformanceTarget, type RouteRequest, type RuntimeAnswer } from "./action-conformance";

const examples = path.resolve(import.meta.dir, "../../examples/product-fit");
function contractOf(name: string): ActionContract {
  return generateActionContract({
    actionsSource: readFileSync(path.join(examples, name, "actions.json"), "utf8"),
    schemaSource: readFileSync(path.join(examples, name, "schema.sql"), "utf8"),
  }).contract;
}

type Quirks = Partial<{
  acceptUnexpected: boolean;
  missingMessage: (key: string) => string;
  extraColumn: boolean;
  nullInNotNull: boolean;
  anyoneMayCall: boolean;
  acceptUnsigned: boolean;
  unsignedStatus: number;
  rateLimitFirst: number;
  /** Answers as a private Deployment: an unsigned route takes a key or a session and is the action's only door. */
  private: boolean;
  /** Answers { changes: 0 } where a guarded mutation should refuse with its guard. */
  dropGuard: boolean;
  /** The caller holds no role: every guarded mutation refuses with its guard, as for a member on an admin's action. */
  withoutRole: boolean;
  /** An API key the backend accepts on a route that takes keys, as the caller's ID. */
  key: string;
  /** Runs a GET route for HEAD instead of refusing it with 405. */
  answerHead: boolean;
}>;

/**
 * A backend that follows the contract the way the Kernel does, with quirks
 * that each break one rule. The runner must pass the honest one and catch
 * every quirk.
 */
function fakeBackend(contract: ActionContract, quirks: Quirks = {}, session = true): ConformanceTarget & { calls: number } {
  let limited = quirks.rateLimitFirst ?? 0;
  const seen = new Set<string>();
  const bind = (action: ContractAction, input: Record<string, unknown>, actor: boolean): string | null => {
    const accepted = action.inputs.map((candidate) => candidate.key);
    const unexpected = Object.keys(input).find((key) => !accepted.includes(key));
    if (unexpected && !quirks.acceptUnexpected) return `Unexpected action input: ${unexpected}`;
    for (const binding of action.bindings) {
      if (binding.source === "actor" && !actor) return "This action requires an authenticated actor.";
      if (binding.source !== "input") continue;
      const spec = action.inputs.find((candidate) => candidate.key === binding.key)!;
      const value = input[spec.key];
      if (spec.nullable && (value === undefined || value === null)) continue;
      if (value === undefined) return quirks.missingMessage?.(spec.key) ?? `Missing action input: ${spec.key}`;
      if (spec.type === "string" && (typeof value !== "string" || !value.trim())) return `${spec.key} must be a non-empty string.`;
      if (spec.type === "string" && (value as string).length > (spec.maxLength ?? 10_000)) return `${spec.key} is too long.`;
      if (spec.type === "number" && typeof value !== "number") return `${spec.key} must be a number.`;
      if (spec.type === "boolean" && typeof value !== "boolean") return `${spec.key} must be a boolean.`;
    }
    return null;
  };
  // A guarded mutation changes a row the first time and nothing after, as an UPDATE with a state check does.
  const ran = new Map<string, number>();
  const answer = (action: ContractAction): unknown => {
    if (action.output.kind === "changes") {
      const times = (ran.get(action.name) ?? 0) + 1;
      ran.set(action.name, times);
      if (action.guard && (times > 1 || quirks.withoutRole)) return quirks.dropGuard ? { changes: 0, lastInsertId: 0 } : { guard: action.guard };
      return { changes: 1, lastInsertId: 7 };
    }
    if (action.output.kind !== "rows") return null;
    const row: Record<string, unknown> = {};
    for (const column of action.output.columns) {
      row[column.name] = quirks.nullInNotNull && column.nullable === false ? null
        : column.type === "integer" ? 1 : column.type === "real" ? 1.5 : "text";
    }
    if (quirks.extraColumn) row.extra = 1;
    return [row];
  };
  const target = {
    calls: 0,
    hasSession: session,
    async invoke(name: string, input: unknown, as: "session" | "visitor"): Promise<RuntimeAnswer> {
      target.calls += 1;
      if (limited > 0) { limited -= 1; return { ok: false, message: "rate-limited" }; }
      const action = contract.actions.find((candidate) => candidate.name === name);
      if (!action) return { ok: false, message: "Server action not declared." };
      const actor = as === "session";
      if ((action.http?.onlyDoor === "always" || (action.http && quirks.private)) && !quirks.anyoneMayCall) {
        return { ok: false, message: `Action ${name} answers only on its HTTP route, ${action.http.method} ${action.http.path}.` };
      }
      // Only the Kernel runs a call's then mutation (ADR-0013).
      if (action.thenOf) return { ok: false, message: `Action ${name} runs only after the call ${action.thenOf} answers.` };
      if (action.access === "authenticated" && !actor && !quirks.anyoneMayCall) return { ok: false, message: "authentication-required" };
      const refused = bind(action, input as Record<string, unknown>, actor || Boolean(quirks.anyoneMayCall));
      if (refused) return { ok: false, message: refused };
      const result = answer(action) as { guard?: string };
      return result?.guard ? { ok: false, message: result.guard } : { ok: true, result };
    },
    async route(request: RouteRequest) {
      target.calls += 1;
      if (request.method === "HEAD" && !quirks.answerHead && contract.actions.some((candidate) => candidate.http?.method === "GET" && candidate.http.path === request.path)) {
        return { status: 405, body: "" };
      }
      const method = request.method === "HEAD" ? "GET" : request.method;
      const action = contract.actions.find((candidate) => candidate.http?.method === method && candidate.http.path === request.path);
      if (!action) return request.method === "POST" ? { status: 405, body: { error: "No route accepts POST on this path." } } : { status: 200, body: "<html>" };
      const http = action.http!;
      const keyed = Boolean(quirks.key) && request.headers?.authorization === `Bearer ${quirks.key}`;
      if (keyed && http.declaredCallers && !http.declaredCallers.includes("key")) return { status: 403, body: { error: "This route does not accept API keys." } };
      if (!keyed && request.headers?.authorization?.startsWith("Bearer lbk_")) return { status: 401, body: { error: "invalid-api-key" } };
      if (http.declaredCallers && !keyed && !(request.session && http.declaredCallers.includes("session"))) return { status: 401, body: { error: "authentication-required" } };
      if (quirks.private && !action.http!.signature && !request.session) return { status: 401, body: { error: "authentication-required" } };
      const signature = action.http!.signature;
      if (signature) {
        const header = request.headers?.[signature.header];
        if (!header && !quirks.acceptUnsigned && !keyed) return { status: quirks.unsignedStatus ?? 401, body: { error: "invalid-signature" } };
        if (header && Math.abs(Number(/t=(\d+)/.exec(header)?.[1]) - 1_700_000_000) > 300) return { status: 401, body: { error: "stale-timestamp" } };
        if (header && seen.has(header)) return { status: 409, body: { error: "replayed-signature" } };
        if (header) seen.add(header);
      }
      let input: Record<string, unknown>;
      if (request.method === "GET") input = Object.fromEntries(Object.entries(request.query ?? {}).map(([key, value]) => {
        const spec = action.inputs.find((candidate) => candidate.key === key);
        return [key, spec?.type === "number" ? Number(value) : value];
      }));
      else {
        try { input = JSON.parse(request.body ?? "{}"); } catch { return { status: 400, body: { error: "The request body must be a JSON object." } }; }
      }
      const refused = bind(action, input, Boolean(request.session) || keyed);
      if (refused) return { status: 400, body: { error: refused } };
      const result = answer(action) as { guard?: string };
      return result?.guard ? { status: 403, body: { error: result.guard } } : { status: 200, body: { result } };
    },
    secrets: { pings_webhook: "s3cret", status_webhook: "s3cret" },
    ...(quirks.key ? { key: quirks.key } : {}),
  };
  return target;
}

const noWait = { wait: async () => {}, nonce: "test", now: () => 1_700_000_000 };
const failures = (report: Awaited<ReturnType<typeof runConformance>>) => report.results.filter((result) => result.outcome === "fail");

describe("conformance runner", () => {
  test("a backend that follows the contract passes every check of every example", async () => {
    for (const name of ["board", "docs", "feed", "journal", "launch", "poll", "portfolio", "probe"]) {
      const contract = contractOf(name);
      const report = await runConformance(contract, fakeBackend(contract), noWait);
      expect({ name, failures: failures(report) }).toEqual({ name, failures: [] });
      expect(report.skipped).toBe(0);
    }
  });

  test("the feed's routes: signed POST, its replay, the unsigned POST and an unbound path", async () => {
    const contract = contractOf("feed");
    const report = await runConformance(contract, fakeBackend(contract), noWait);
    const checks = report.results.filter((result) => result.action === "pings.receive" || result.action === "routes").map((result) => `${result.outcome} ${result.check}`);
    expect(checks).toContain("pass answers POST /hooks/pings");
    expect(checks).toContain("pass refuses a replayed signature");
    expect(checks).toContain("pass refuses an unsigned request");
    expect(checks).toContain("pass refuses a body that is not JSON");
    expect(checks).toContain("pass refuses a POST to a path no action binds");
    expect(report.results.find((result) => result.action === "incidents.recent" && result.check === "refuses HEAD")).toMatchObject({ outcome: "pass", detail: "Answered 405; a HEAD answer carries no body." });
    expect(report.results.some((result) => result.action === "pings.receive" && result.check === "refuses HEAD")).toBe(false);
  });

  const cases: [string, string, Quirks, RegExp][] = [
    ["accepts an input the action does not declare", "launch", { acceptUnexpected: true }, /Answered .* where the contract refuses with "Unexpected action input: conformProbe"/],
    ["refuses a missing input with another message", "launch", { missingMessage: (key) => `${key} must be a non-empty string.` }, /Refused with "email must be a non-empty string\." where the contract says "Missing action input: email"/],
    ["returns a column the contract does not list", "journal", { extraColumn: true }, /has the keys id, body, mood, created_at, extra/],
    ["returns null in a column the schema declares NOT NULL", "poll", { nullInNotNull: true }, /n is null where the contract says it is not nullable/],
    ["runs an authenticated action for a visitor", "journal", { anyoneMayCall: true }, /where the contract refuses with "authentication-required"/],
    ["accepts an unsigned webhook", "feed", { acceptUnsigned: true }, /Answered 200 .* where the contract says 401 "invalid-signature"/],
    ["refuses an unsigned webhook with the wrong status", "feed", { unsignedStatus: 403 }, /Answered 403 "invalid-signature" where the contract says 401/],
    ["runs a GET route for HEAD", "feed", { answerHead: true }, /Answered 200 where the contract says 405/],
  ];
  for (const [quirk, example, quirks, message] of cases) {
    test(`fails a backend that ${quirk}`, async () => {
      const contract = contractOf(example);
      const report = await runConformance(contract, fakeBackend(contract, quirks), noWait);
      expect(report.failed).toBeGreaterThan(0);
      expect(failures(report).map((result) => result.detail).join("\n")).toMatch(message);
    });
  }

  test("fails when the contract does not list the refusal it probes", async () => {
    const contract = contractOf("launch");
    const join = contract.actions.find((action) => action.name === "waitlist.join")!;
    join.errors = join.errors.filter((error) => !error.message.startsWith("Missing action input"));
    const report = await runConformance(contract, fakeBackend(contract), noWait);
    expect(failures(report).map((result) => result.detail)).toContain('The contract does not list "Missing action input: email" for this action.');
  });

  test("skips, and does not pass, what needs a session it was not given", async () => {
    const contract = contractOf("journal");
    const report = await runConformance(contract, fakeBackend(contract, {}, false), noWait);
    expect(report.failed).toBe(0);
    expect(report.results.filter((result) => result.outcome === "skip").map((result) => `${result.action}: ${result.check}`)).toEqual([
      "entries.create: answers a valid call", "entries.create: refuses bad input",
      "entries.delete: answers a valid call", "entries.delete: refuses bad input",
      "entries.list: answers a valid call", "entries.list: refuses bad input",
    ]);
    // Without a session, the access refusal is still checked.
    expect(report.results.filter((result) => result.check === "refuses a caller without a session" && result.outcome === "pass")).toHaveLength(3);
  });

  test("on a private Deployment, an unsigned route's action answers only there, and the route asks for a key or a session", async () => {
    const contract = contractOf("feed");
    const visitor = await runConformance(contract, fakeBackend(contract, { private: true }, false), noWait);
    expect(failures(visitor)).toEqual([]);
    const recent = visitor.results.filter((result) => result.action === "incidents.recent").map((result) => `${result.outcome} ${result.check}`);
    expect(recent).toContain("pass refuses a caller without a key or a session");
    expect(recent).toContain("skip answers GET /api/incidents");
    const member = await runConformance(contract, fakeBackend(contract, { private: true }), noWait);
    expect(failures(member)).toEqual([]);
    const withSession = member.results.filter((result) => result.action === "incidents.recent").map((result) => `${result.outcome} ${result.check}`);
    expect(withSession).toContain("pass answers only on its route on this private Deployment");
    expect(withSession).toContain("pass answers GET /api/incidents with a session");
  });

  test("a signed route answers only on its route, and its stale timestamp is probed", async () => {
    const contract = contractOf("feed");
    const report = await runConformance(contract, fakeBackend(contract), noWait);
    const checks = report.results.filter((result) => result.action === "pings.receive").map((result) => `${result.outcome} ${result.check}`);
    expect(checks).toContain("pass answers only on its route");
    expect(checks).toContain("pass refuses a stale timestamp");
  });

  test("a guarded mutation must answer its guard when nothing changes, never { changes: 0 }", async () => {
    const { contract } = generateActionContract({
      schemaSource: "CREATE TABLE IF NOT EXISTS members (id TEXT PRIMARY KEY, role TEXT NOT NULL);",
      actionsSource: JSON.stringify({ version: 1, actions: {
        "members.promote": {
          kind: "mutation", access: "authenticated", guard: "Only an organizer can promote a member.",
          statement: "UPDATE members SET role = 'organizer' WHERE id = ?1 AND EXISTS (SELECT 1 FROM members WHERE id = ?2 AND role = 'organizer')",
          bindings: [{ source: "input", key: "member", type: "string", maxLength: 64 }, { source: "actor", field: "id" }],
        },
      } }),
    });
    const promote = contract.actions[0];
    expect(promote.guard).toBe("Only an organizer can promote a member.");
    expect(promote.errors.find((error) => error.message === promote.guard)).toMatchObject({ via: "runtime" });
    const honest = await runConformance(contract, fakeBackend(contract), noWait);
    expect(failures(honest)).toEqual([]);
    expect(honest.results.find((result) => result.check === "refuses with its guard when no row changes")).toMatchObject({ outcome: "pass" });
    const dropped = await runConformance(contract, fakeBackend(contract, { dropGuard: true }), noWait);
    expect(failures(dropped).map((result) => result.detail)).toEqual(['Answered {"changes":0,"lastInsertId":0} where the contract refuses with "Only an organizer can promote a member.".']);
  });

  test("the starter's member, who holds no role, passes every guard, the nullable probes included", async () => {
    const starter = path.resolve(import.meta.dir, "../../examples/showcase/starter");
    const { contract } = generateActionContract({
      actionsSource: readFileSync(path.join(starter, "actions.json"), "utf8"),
      schemaSource: readFileSync(path.join(starter, "schema.sql"), "utf8"),
    });
    const publish = contract.actions.find((action) => action.name === "updates.publish")!;
    expect(publish.guard).toBe("Only team admins can publish an update.");
    expect(publish.inputs.find((input) => input.key === "body")?.nullable).toBe(true);
    const member = await runConformance(contract, fakeBackend(contract, { withoutRole: true, key: "lbk_starter_conform" }), noWait);
    expect(failures(member)).toEqual([]);
    const checks = member.results.filter((result) => result.action === "updates.publish").map((result) => `${result.outcome} ${result.check}: ${result.detail}`);
    expect(checks).toContain('pass answers without the nullable body: Refused with its guard, "Only team admins can publish an update.": the bindings accepted the call without body.');
    expect(checks).toContain('pass refuses with its guard when no row changes: Refused with "Only team admins can publish an update.".');
    // A backend that drops the guard still fails, and so does another refusal on the nullable probe.
    const dropped = await runConformance(contract, fakeBackend(contract, { withoutRole: true, dropGuard: true }), noWait);
    expect(failures(dropped).some((result) => result.action === "updates.publish")).toBe(true);
    const strict = fakeBackend(contract, { withoutRole: true });
    const invoke = strict.invoke.bind(strict);
    strict.invoke = async (name, input, as) => name === "updates.publish" && !("body" in (input as object)) ? { ok: false, message: "Missing action input: body" } : invoke(name, input, as);
    const refusedProbe = await runConformance(contract, strict, noWait);
    expect(failures(refusedProbe).map((result) => `${result.action}: ${result.check}`)).toEqual(["updates.publish: answers without the nullable body"]);
  });

  test("an actor-bound route is called with --key, whose ID is the actor, and is not skipped", async () => {
    const { contract } = generateActionContract({
      schemaSource: "CREATE TABLE IF NOT EXISTS updates (id INTEGER PRIMARY KEY, title TEXT NOT NULL, author_id TEXT NOT NULL);",
      actionsSource: JSON.stringify({ version: 1, actions: {
        "updates.post": {
          kind: "mutation", access: "anonymous", http: { method: "POST", path: "/api/updates", callers: ["key"] },
          statement: "INSERT INTO updates (title, author_id) VALUES (?1, ?2)",
          bindings: [{ source: "input", key: "title", type: "string", maxLength: 120 }, { source: "actor", field: "id" }],
        },
        "updates.mine": {
          kind: "query", access: "anonymous", http: { method: "GET", path: "/api/mine" },
          statement: "SELECT id, title FROM updates WHERE author_id = ?1 ORDER BY id DESC LIMIT 20",
          bindings: [{ source: "actor", field: "id" }],
        },
      } }),
    });
    expect(contract.actions.every((action) => action.actorBound)).toBe(true);
    const keyOnly = await runConformance(contract, fakeBackend(contract, { key: "lbk_conform" }, false), noWait);
    expect(failures(keyOnly)).toEqual([]);
    const lines = keyOnly.results.map((result) => `${result.outcome} ${result.action}: ${result.check}`);
    expect(lines).toContain("pass updates.post: answers POST /api/updates with an API key");
    expect(lines).toContain("pass updates.mine: answers GET /api/mine with an API key");
    expect(lines).toContain("pass updates.post: refuses an undeclared input on the route");
    expect(keyOnly.results.filter((result) => result.outcome === "skip").map((result) => `${result.action}: ${result.check}`)).toEqual([
      // A key works only on routes; the runtime endpoint needs a session.
      "updates.mine: answers a valid call", "updates.mine: refuses bad input",
    ]);
    const without = await runConformance(contract, fakeBackend(contract, {}, false), noWait);
    expect(without.results.filter((result) => result.check.startsWith("answers POST") || result.check.startsWith("answers GET")).map((result) => `${result.outcome} ${result.detail}`))
      .toEqual(["skip Needs an API key or a session (--key or --token).", "skip Needs an API key or a session (--key or --token)."]);
  });

  test("waits out a rate limit and sends the call again", async () => {
    const contract = contractOf("launch");
    const waits: number[] = [];
    const report = await runConformance(contract, fakeBackend(contract, { rateLimitFirst: 2 }), { ...noWait, wait: async (ms) => { waits.push(ms); } });
    expect(report.failed).toBe(0);
    expect(report.rateLimitWaits).toBe(2);
    expect(waits).toEqual([10_000, 10_000]);
    expect(formatConformanceReport(report)).toMatch(/passed, 0 failed, 0 skipped, 2 rate-limit waits\.$/);
  });

  test("checks a mutation's answer has exactly changes and lastInsertId", () => {
    const action = contractOf("launch").actions.find((candidate) => candidate.kind === "mutation")!;
    expect(checkOutput(action, { changes: 1, lastInsertId: 3 })).toBeNull();
    expect(checkOutput(action, { changes: 1 })).toMatch(/keys changes where the contract says changes, lastInsertId/);
    expect(checkOutput(action, [{ id: 1 }])).toMatch(/where the contract says \{ changes, lastInsertId \}/);
  });
});

describe("example inputs", () => {
  test("a statement error on a valid call fails with advice, and --inputs supplies a value that works", async () => {
    const contract = contractOf("probe");
    const backend = fakeBackend(contract);
    const invoke = backend.invoke.bind(backend);
    backend.invoke = async (name, input, as) => {
      const ids = (input as Record<string, unknown>)?.ids;
      // Like the Kernel, the bindings are checked before SQLite reads the value.
      const answer = await invoke(name, input, as);
      if (answer.ok && name === "items.jsonin" && typeof ids === "string" && !ids.startsWith("[")) return { ok: false, message: "malformed JSON" };
      return answer;
    };
    const generated = await runConformance(contract, backend, noWait);
    expect(failures(generated).map((result) => `${result.action}: ${result.detail}`)).toEqual([
      'items.jsonin: Refused with "malformed JSON". The contract lists no such refusal, so the statement itself failed. If an input needs a particular form, such as JSON, give one with --inputs.',
    ]);
    const given = await runConformance(contract, backend, { ...noWait, inputs: { "items.jsonin": { ids: "[1]" } } });
    expect(given.failed).toBe(0);
  });
});
