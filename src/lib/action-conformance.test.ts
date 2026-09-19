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
  const answer = (action: ContractAction): unknown => {
    if (action.output.kind === "changes") return { changes: 1, lastInsertId: 7 };
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
      if (action.access === "authenticated" && !actor && !quirks.anyoneMayCall) return { ok: false, message: "authentication-required" };
      const refused = bind(action, input as Record<string, unknown>, actor || Boolean(quirks.anyoneMayCall));
      return refused ? { ok: false, message: refused } : { ok: true, result: answer(action) };
    },
    async route(request: RouteRequest) {
      target.calls += 1;
      const action = contract.actions.find((candidate) => candidate.http?.method === request.method && candidate.http.path === request.path);
      if (!action) return request.method === "POST" ? { status: 405, body: { error: "No route accepts POST on this path." } } : { status: 200, body: "<html>" };
      if (request.headers?.authorization?.startsWith("Bearer lbk_")) return { status: 401, body: { error: "invalid-api-key" } };
      const signature = action.http!.signature;
      if (signature) {
        const header = request.headers?.[signature.header];
        if (!header && !quirks.acceptUnsigned) return { status: quirks.unsignedStatus ?? 401, body: { error: "invalid-signature" } };
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
      const refused = bind(action, input, false);
      return refused ? { status: 400, body: { error: refused } } : { status: 200, body: { result: answer(action) } };
    },
    secrets: { pings_webhook: "s3cret" },
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
  });

  const cases: [string, string, Quirks, RegExp][] = [
    ["accepts an input the action does not declare", "launch", { acceptUnexpected: true }, /Answered .* where the contract refuses with "Unexpected action input: conformProbe"/],
    ["refuses a missing input with another message", "launch", { missingMessage: (key) => `${key} must be a non-empty string.` }, /Refused with "email must be a non-empty string\." where the contract says "Missing action input: email"/],
    ["returns a column the contract does not list", "journal", { extraColumn: true }, /has the keys id, body, mood, created_at, extra/],
    ["returns null in a column the schema declares NOT NULL", "poll", { nullInNotNull: true }, /n is null where the contract says it is not nullable/],
    ["runs an authenticated action for a visitor", "journal", { anyoneMayCall: true }, /where the contract refuses with "authentication-required"/],
    ["accepts an unsigned webhook", "feed", { acceptUnsigned: true }, /Answered 200 .* where the contract says 401 "invalid-signature"/],
    ["refuses an unsigned webhook with the wrong status", "feed", { unsignedStatus: 403 }, /Answered 403 "invalid-signature" where the contract says 401/],
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
