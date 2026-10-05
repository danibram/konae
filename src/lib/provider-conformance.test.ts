import { describe, expect, test } from "bun:test";
import { createHmac } from "node:crypto";
import { parseServerActionSource } from "../kernel/actions";
import { checkActionContract, generateActionContract, programFromContract, renderActionContractMarkdown, type ActionContract } from "./action-contract";
import { runConformance, type ConformanceTarget, type RouteRequest } from "./action-conformance";

// An app with one route per provider scheme, each reading a nested body through fields.
const actionsSource = JSON.stringify({
  version: 1,
  actions: {
    "issues.receive": {
      kind: "mutation", access: "anonymous",
      http: {
        method: "POST", path: "/hooks/github",
        signature: { scheme: "hmac-body", header: "X-Hub-Signature-256", secret: "github_webhook", algorithm: "sha256", encoding: "hex", prefix: "sha256=" },
        fields: { id: "$.issue.node_id", title: "$.issue.title" },
      },
      statement: "INSERT INTO events (source, external_id, title) VALUES ('github', ?1, ?2)",
      bindings: [{ source: "input", key: "id", type: "string", maxLength: 100 }, { source: "input", key: "title", type: "string", maxLength: 200 }],
    },
    "payments.receive": {
      kind: "mutation", access: "anonymous",
      http: {
        method: "POST", path: "/hooks/payments",
        signature: { scheme: "timestamped", header: "Stripe-Signature", secret: "payments_webhook", prefix: "t=", tag: "v1=", window: 120 },
        fields: { id: "$.id", title: "$.data.object.failure_message" },
      },
      statement: "INSERT INTO events (source, external_id, title) VALUES ('payments', ?1, ?2)",
      bindings: [{ source: "input", key: "id", type: "string", maxLength: 100 }, { source: "input", key: "title", type: "string", maxLength: 200, nullable: true }],
    },
    "emails.receive": {
      kind: "mutation", access: "anonymous",
      http: { method: "POST", path: "/hooks/email", signature: { scheme: "svix", secret: "email_webhook" }, fields: { id: "$.data.email_id" } },
      statement: "INSERT INTO events (source, external_id, title) VALUES ('email', ?1, ?2)",
      bindings: [{ source: "input", key: "id", type: "string", maxLength: 100 }, { source: "input", key: "type", type: "string", maxLength: 60 }],
    },
  },
});
const schemaSource = "CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, source TEXT NOT NULL, external_id TEXT NOT NULL, title TEXT);";
const secrets = { github_webhook: "github-secret-0123456", payments_webhook: "whsec_payments_0123456789", email_webhook: `whsec_${Buffer.from("email-key-0123456789").toString("base64")}` };
const NOW = 1_700_000_000;

function contract(): ActionContract {
  return generateActionContract({ actionsSource, schemaSource }).contract;
}

/**
 * A backend that verifies each scheme as the provider's documentation says,
 * with node:crypto rather than the signer the runner uses, and reads inputs
 * from the paths the contract names.
 */
function providerBackend(subject: ActionContract): ConformanceTarget {
  const seen = new Set<string>();
  const hmac = (key: string | Buffer, message: string, encoding: "hex" | "base64") => createHmac("sha256", key).update(message).digest(encoding);
  const verify = (path: string, request: RouteRequest): { status: number; error: string } | null => {
    const header = (name: string) => Object.entries(request.headers ?? {}).find(([key]) => key.toLowerCase() === name.toLowerCase())?.[1];
    const body = request.body ?? "";
    if (path === "/hooks/github") {
      const sent = header("X-Hub-Signature-256");
      if (sent !== `sha256=${hmac(secrets.github_webhook, body, "hex")}`) return { status: 401, error: "invalid-signature" };
      return seen.has(`gh:${sent}`) ? { status: 409, error: "replayed-signature" } : (seen.add(`gh:${sent}`), null);
    }
    if (path === "/hooks/payments") {
      const fields = Object.fromEntries((header("Stripe-Signature") ?? "").split(",").map((part) => part.split("=") as [string, string]));
      if (!fields.t) return { status: 401, error: "invalid-signature" };
      if (Math.abs(Number(fields.t) - NOW) > 120) return { status: 401, error: "stale-timestamp" };
      if (fields.v1 !== hmac(secrets.payments_webhook, `${fields.t}.${body}`, "hex")) return { status: 401, error: "invalid-signature" };
      return seen.has(`pay:${fields.v1}`) ? { status: 409, error: "replayed-signature" } : (seen.add(`pay:${fields.v1}`), null);
    }
    const [id, t, signature] = [header("svix-id"), header("svix-timestamp"), header("svix-signature")];
    if (!id || !t || !signature) return { status: 401, error: "invalid-signature" };
    if (Math.abs(Number(t) - NOW) > 300) return { status: 401, error: "stale-timestamp" };
    const key = Buffer.from(secrets.email_webhook.slice("whsec_".length), "base64");
    if (signature !== `v1,${hmac(key, `${id}.${t}.${body}`, "base64")}`) return { status: 401, error: "invalid-signature" };
    return seen.has(`svix:${id}`) ? { status: 409, error: "replayed-signature" } : (seen.add(`svix:${id}`), null);
  };
  const at = (value: unknown, path: string) => path.slice(1).split(".").filter(Boolean).reduce<unknown>((node, key) => (node as Record<string, unknown> | undefined)?.[key], value);
  return {
    hasSession: false,
    secrets,
    async invoke(name) {
      const action = subject.actions.find((candidate) => candidate.name === name);
      return { ok: false, message: action?.http ? `Action ${name} answers only on its HTTP route, ${action.http.method} ${action.http.path}.` : "Server action not declared." };
    },
    async route(request) {
      const action = subject.actions.find((candidate) => candidate.http?.path === request.path);
      if (!action) return { status: 405, body: { error: "No route accepts POST on this path." } };
      if (request.headers?.authorization?.startsWith("Bearer lbk_")) return { status: 401, body: { error: "invalid-api-key" } };
      const refused = verify(request.path, request);
      if (refused) return { status: refused.status, body: { error: refused.error } };
      let body: Record<string, unknown>;
      try { body = JSON.parse(request.body ?? ""); } catch { return { status: 400, body: { error: "The request body must be a JSON object." } }; }
      for (const input of action.inputs) {
        const path = action.http!.fields?.[input.key];
        const value = path ? at(body, path) : body[input.key];
        if (value === undefined && !input.nullable) return { status: 400, body: { error: `Missing action input: ${input.key}` } };
        if (value !== undefined && typeof value !== "string") return { status: 400, body: { error: `${input.key} must be a non-empty string.` } };
      }
      return { status: 200, body: { result: { changes: 1, lastInsertId: 1 } } };
    },
  };
}

describe("a route in a provider's scheme", () => {
  test("the Action Contract names the scheme, its headers, its window and the fields, and round-trips", () => {
    const subject = contract();
    expect(checkActionContract(JSON.parse(JSON.stringify(subject)))).toEqual([]);
    const [emails, issues, payments] = ["emails.receive", "issues.receive", "payments.receive"].map((name) => subject.actions.find((action) => action.name === name)!);
    expect(issues.http!.signature).toMatchObject({ scheme: "hmac-body", header: "X-Hub-Signature-256", headers: ["X-Hub-Signature-256"], windowSeconds: null, algorithm: "sha256", encoding: "hex", prefix: "sha256=" });
    expect(issues.http!.signature!.format).toBe("sha256=<hex HMAC-SHA256, keyed with the secret, of the raw body>");
    expect(payments.http!.signature).toMatchObject({ scheme: "timestamped", windowSeconds: 120, prefix: "t=", tag: "v1=" });
    expect(emails.http!.signature).toMatchObject({ scheme: "svix", headers: ["svix-id", "svix-timestamp", "svix-signature"], windowSeconds: 300 });
    expect(issues.http!.fields).toEqual({ id: "$.issue.node_id", title: "$.issue.title" });
    expect(issues.http!.input).toContain("id is read from $.issue.node_id, title is read from $.issue.title, and the body's other keys are left out");
    // A body-only MAC has no time to be stale, and a route with fields never answers an undeclared key.
    const messages = (action: typeof issues) => action.errors.map((error) => error.message);
    expect(messages(issues)).not.toContain("stale-timestamp");
    expect(messages(payments)).toContain("stale-timestamp");
    expect(messages(issues)).not.toContain("Unexpected action input: <key>");
    expect(messages(issues)).toContain("replayed-signature");
    const markdown = renderActionContractMarkdown(subject);
    expect(markdown).toContain("uses the timestamped scheme and carries `Stripe-Signature: t=<unix seconds>,v1=<hex HMAC-SHA256, keyed with the secret, of <t>.<raw body>>`");
    expect(markdown).toContain("The scheme carries no time, and each MAC is accepted once in 72 hours");
    expect(programFromContract(subject)).toEqual(parseServerActionSource(actionsSource));
  });

  test("conform signs each probe in the route's scheme and nests the body at the declared paths", async () => {
    const subject = contract();
    const report = await runConformance(subject, providerBackend(subject), { wait: async () => {}, nonce: "test", now: () => NOW });
    expect(report.results.filter((result) => result.outcome === "fail")).toEqual([]);
    expect(report.skipped).toBe(0);
    const checks = report.results.map((result) => `${result.outcome} ${result.action}: ${result.check}`);
    for (const name of ["issues.receive", "payments.receive", "emails.receive"]) {
      expect(checks).toContain(`pass ${name}: refuses a replayed signature`);
      expect(checks).toContain(`pass ${name}: refuses an unsigned request`);
      expect(checks).not.toContain(`pass ${name}: refuses an undeclared input on the route`);
    }
    expect(checks).toContain("pass payments.receive: refuses a stale timestamp");
    expect(checks).toContain("pass emails.receive: refuses a stale timestamp");
    expect(checks.some((check) => check.includes("issues.receive: refuses a stale timestamp"))).toBe(false);
  });

  test("conform fails a backend that ignores the declared path", async () => {
    const subject = contract();
    const honest = providerBackend(subject);
    const flat: ConformanceTarget = {
      ...honest,
      async route(request) {
        if (request.path !== "/hooks/github") return honest.route!(request);
        const answer = await honest.route!(request);
        // Reads the top-level key, as a route without fields does: the nested body has none.
        return answer.status === 200 ? { status: 400, body: { error: "Missing action input: id" } } : answer;
      },
    };
    const report = await runConformance(subject, flat, { wait: async () => {}, nonce: "test", now: () => NOW });
    expect(report.results.filter((result) => result.outcome === "fail").map((result) => `${result.action}: ${result.check}`)).toContain("issues.receive: answers POST /hooks/github");
  });
});
