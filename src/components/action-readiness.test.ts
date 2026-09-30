import { describe, expect, test } from "bun:test";
import { cloneStarter } from "../lib/starter";
import { actionReadout, actionSentence, deployReadiness, refusalNeedsSecret } from "./action-readiness";

function withActions(actions: Record<string, unknown>) {
  const project = cloneStarter();
  project.files["actions.json"] = JSON.stringify({ version: 1, actions });
  return project;
}

describe("the Deploy dialog's readiness", () => {
  test("lets a total with no LIMIT leave, as both doors do", () => {
    const project = withActions({ "orders.count": { kind: "query", access: "anonymous", statement: "SELECT count(*) AS n FROM orders", bindings: [] } });
    const { refusal, actions } = deployReadiness(project);
    expect(refusal).toBeNull();
    expect(actionSentence(actions[0], "public")).toBe("Anyone can run it. The Kernel stops it at 1000 rows.");
  });

  test("refuses a LIMIT that sits in a string with the Kernel's sentence", () => {
    const project = withActions({ "orders.list": { kind: "query", access: "anonymous", statement: "SELECT id FROM orders WHERE note = 'LIMIT 5'", bindings: [] } });
    expect(deployReadiness(project).refusal).toBe(
      "Action orders.list reads rows and has no LIMIT. Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.",
    );
  });

  test("says why actions.json cannot publish instead of blaming a LIMIT", () => {
    const project = withActions({ "orders.list": { kind: "query", access: "anonymous", statement: "SELECT id FROM orders LIMIT 5", bindings: [], guard: "No." } });
    expect(deployReadiness(project).refusal).toBe("Action orders.list is a query, so it cannot declare a guard. Declare it on a mutation.");
  });

  test("names the row count the query writes, not a fixed 100", () => {
    const { actions } = deployReadiness(withActions({
      "orders.recent": { kind: "query", access: "authenticated", statement: "SELECT id FROM orders ORDER BY id DESC LIMIT 20", bindings: [] },
      "orders.one": { kind: "query", access: "anonymous", statement: "SELECT id FROM orders LIMIT 1", bindings: [] },
      "orders.add": { kind: "mutation", access: "anonymous", statement: "INSERT INTO orders (id) VALUES (1)", bindings: [] },
      "page.like": { kind: "signal", access: "anonymous", bindings: [], signal: { name: "page.liked" } },
    }));
    expect(actions.map((action) => actionSentence(action, "public"))).toEqual([
      "A signed-in person can run it. The list stops at 20 rows.",
      "Anyone can run it. The list stops at 1 row.",
      "Anyone can run it.",
      "Anyone can run it. It records a Signal and writes no row.",
    ]);
  });
});

describe("the readout above actions.json", () => {
  test("shows the parser's refusal word for word, LIMIT or not", () => {
    const unbounded = JSON.stringify({ version: 1, actions: { "orders.list": { kind: "query", access: "anonymous", statement: "SELECT id FROM orders", bindings: [] } } });
    expect(actionReadout(unbounded, "public")).toEqual({
      refusal: "Action orders.list reads rows and has no LIMIT. Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.",
      lines: [],
    });
  });

  test("does not call a total refused", () => {
    const total = JSON.stringify({ version: 1, actions: { "orders.count": { kind: "query", access: "anonymous", statement: "SELECT count(*) AS n FROM orders", bindings: [] } } });
    expect(actionReadout(total, "public")).toEqual({ refusal: null, lines: ["orders.count. Anyone can run it. The Kernel stops it at 1000 rows."] });
  });
});

describe("who an action line names", () => {
  const route = { name: "orders.receive", kind: "mutation", access: "anonymous" as const, statement: "INSERT INTO orders (id) VALUES (1)",
    http: { method: "POST" as const, path: "/hooks/orders", signature: { header: "X-Signature", secret: "orders_webhook" } } };

  test("a signed route is the action's only door, on either visibility", () => {
    for (const visibility of ["public", "private"] as const) {
      expect(actionSentence(route, visibility)).toBe("Only its route, POST /hooks/orders, runs it, for a signed request or an API key.");
    }
  });

  test("an unsigned route answers anyone on a public app and keys or sessions on a private one", () => {
    const open = { ...route, http: { method: "POST" as const, path: "/hooks/orders" } };
    expect(actionSentence(open, "public")).toBe("Anyone can run it. It also answers on POST /hooks/orders.");
    expect(actionSentence(open, "private")).toBe("Only its route, POST /hooks/orders, runs it, for an API key or a signed-in person.");
  });

  test("an anonymous action on a private app needs a session", () => {
    expect(actionSentence({ name: "tasks.list", kind: "query", access: "anonymous", statement: "SELECT id FROM tasks LIMIT 5" }, "private"))
      .toBe("A signed-in person can run it. The list stops at 5 rows.");
  });

  test("a guard says what it answers", () => {
    expect(actionSentence({ name: "tasks.toggle", kind: "mutation", access: "anonymous", statement: "UPDATE tasks SET done = 1 WHERE id = ?", guard: "That task is gone." }, "public"))
      .toBe('Anyone can run it. When no row changes it refuses with "That task is gone.".');
  });
});

describe("a publish refusal that needs a secret", () => {
  test("matches the Kernel's sentences for one or several secrets and nothing else", () => {
    expect(refusalNeedsSecret("Action orders.receive signs with the secret orders_webhook, which is not set for this project. Set it with secret_set before publishing.")).toBe(true);
    expect(refusalNeedsSecret("Actions a.one and b.two sign with the secret hook, which is not set for this project. Set it with secret_set before publishing.")).toBe(true);
    expect(refusalNeedsSecret("Actions a.one, c.three and d.four sign with the secrets feed and hook, which are not set for this project. Set each with secret_set before publishing.")).toBe(true);
    expect(refusalNeedsSecret("Action orders.list reads rows and has no LIMIT.")).toBe(false);
  });
});
