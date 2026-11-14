import { describe, expect, test } from "bun:test";
import { cloneStarter } from "../lib/starter";
import { actionReadout, actionSentence, actionTitle, actorFiltered, deployReadiness, namedSecrets, ownerFixes, refusedSpot } from "./action-readiness";

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
    expect(actionSentence(actions[0], "public")).toBe("Anyone can run it. It returns one number.");
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
    expect(actionReadout(total, "public")).toEqual({ refusal: null, lines: ["Count. Anyone can run it. It returns one number."] });
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

describe("a publish refusal that needs the owner", () => {
  test("follows the code's fix in kernel-rs/kernel/src/refusal-codes.json, not the sentence", () => {
    for (const code of ["route.signature-secret-unset", "route.signature-secret-call-only", "route.signature-svix-secret-invalid", "call.secret-unset"]) {
      expect(ownerFixes(code), code).toBe(true);
    }
    for (const code of ["query.limit-missing", "call.http-off-installation", "call.host-editor", undefined]) expect(ownerFixes(code), String(code)).toBe(false);
  });
});


describe("the Deploy dialog before release_plan", () => {
  const signed = {
    "orders.receive": { kind: "mutation", access: "anonymous", statement: "INSERT INTO tasks (title) VALUES (?1)",
      bindings: [{ source: "input", key: "title", type: "string" }],
      http: { method: "POST", path: "/hooks/orders", signature: { header: "X-Signature", secret: "orders_webhook" } } },
    "hook.send": { kind: "call", access: "anonymous", method: "POST", url: "https://api.example.com/send", bindings: [], auth: { key: "api_key" } },
  };
  const rules = { suffix: "apps.example.org", installationHosts: ["editor.example.org"] };

  test("says nothing about secrets it could not read", () => {
    expect(deployReadiness(withActions(signed)).refusal).toBeNull();
    expect(deployReadiness(withActions(signed), { rules }).refusal).toBeNull();
  });

  test("refuses an unset secret before Deploy and says the owner acts", () => {
    const answer = deployReadiness(withActions(signed), { secrets: [{ name: "api_key", use: "call" }], rules });
    expect(answer.refusal).toBe("Action orders.receive signs with the secret orders_webhook, which is not set for this project. Set it with secret_set before publishing.");
    expect(answer.ownerActs).toBe(true);
    expect(answer.actions).toHaveLength(2);
  });

  test("refuses a call to the editor as an edit, not an owner step", () => {
    const editor = { ...signed, "hook.send": { ...signed["hook.send"], url: "https://editor.example.org/x" } };
    const answer = deployReadiness(withActions(editor), { secrets: [{ name: "orders_webhook", use: "route" }, { name: "api_key", use: "call" }], rules });
    expect(answer.refusal).toBe("Action hook.send calls editor.example.org, the editor's host. A call reaches an app or an outside service, never the editor.");
    expect(answer.ownerActs).toBe(false);
  });

  test("passes when every named secret is set and every call leaves the editor", () => {
    expect(deployReadiness(withActions(signed), { secrets: [{ name: "orders_webhook" }, { name: "api_key", use: "call" }], rules }))
      .toMatchObject({ refusal: null, ownerActs: false });
  });

  test("lists every secret actions.json names, from routes and calls", () => {
    const named = namedSecrets(JSON.stringify({ version: 1, actions: signed }));
    expect([...named.entries()]).toEqual([
      ["api_key", { actions: ["hook.send"], route: false }],
      ["orders_webhook", { actions: ["orders.receive"], route: true }],
    ]);
    expect(namedSecrets("not json").size).toBe(0);
  });
});

describe("refusedSpot", () => {
  const source = JSON.stringify({ version: 1, actions: {
    "notes.create": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes (title) VALUES (?1)", bindings: [] },
    "notes.list": { kind: "query", access: "authenticated", statement: "SELECT id FROM notes", bindings: [] },
  } }, null, 2);

  test("points at the statement of the action the refusal names", () => {
    const spot = refusedSpot(source, "Action notes.list reads rows without a LIMIT. Add a LIMIT.");
    expect(spot?.action).toBe("notes.list");
    expect(source.slice(spot!.index, spot!.index + 11)).toBe('"statement"');
    expect(spot!.index).toBeGreaterThan(source.indexOf('"notes.list"'));
    expect(source.slice(spot!.value!.start, spot!.value!.end)).toBe("SELECT id FROM notes");
  });

  test("the statement's span ends at its closing quote, past escaped quotes", () => {
    const escaped = JSON.stringify({ version: 1, actions: { "notes.list": { kind: "query", access: "authenticated", statement: 'SELECT id FROM notes WHERE title = "a"', bindings: [] } } }, null, 2);
    const spot = refusedSpot(escaped, "Action notes.list reads rows and has no LIMIT.");
    expect(JSON.parse(`"${escaped.slice(spot!.value!.start, spot!.value!.end)}"`)).toBe('SELECT id FROM notes WHERE title = "a"');
  });

  test("says nothing when the refusal names no action in the file", () => {
    expect(refusedSpot(source, "Unexpected token } in JSON at position 3")).toBeNull();
    expect(refusedSpot(source, "Action tasks.list has an invalid kind.")).toBeNull();
    expect(refusedSpot(source, null)).toBeNull();
  });
});

describe("an action's sentence says only what its statement proves", () => {
  const actor = { source: "actor" };
  const input = { source: "input" };
  const query = (statement: string, bindings: Array<{ source: string }> = []) => ({ name: "x.list", kind: "query", access: "authenticated" as const, statement, bindings });

  test("a total with no GROUP BY returns one number, and several totals one row", () => {
    expect(actionSentence(query("SELECT count(*) AS n FROM messages"), "private")).toBe("A signed-in person can run it. It returns one number.");
    expect(actionSentence(query("SELECT count(*) AS n, max(created_at) AS last FROM messages"), "private")).toBe("A signed-in person can run it. It returns one row of totals.");
  });

  test("a select with no table returns one row", () => {
    expect(actionSentence(query("SELECT 1 AS ok"), "private")).toBe("A signed-in person can run it. It returns one row.");
  });

  test("a LIMIT from a binding says the Kernel's ceiling", () => {
    expect(actionSentence(query("SELECT id FROM notes LIMIT ?1", [input]), "private")).toBe("A signed-in person can run it. It returns at most 1000 rows.");
  });

  test("a top-level filter on the actor reads their own rows", () => {
    expect(actionSentence(query("SELECT id, title FROM notes WHERE user_id = ?1 ORDER BY id DESC LIMIT 50", [actor]), "private"))
      .toBe("A signed-in person can run it. The list stops at 50 rows. It reads only their own rows.");
    expect(actionSentence(query("SELECT id FROM notes WHERE archived = 0 AND notes.owner = ?2 LIMIT 5", [input, actor]), "private"))
      .toBe("A signed-in person can run it. The list stops at 5 rows. It reads only their own rows.");
  });

  test("a total counted over the actor's rows says both, and never 'their own account'", () => {
    const me = { ...query("SELECT count(*) AS admin FROM admins WHERE user_id = ?1", [actor]), name: "admins.me" };
    expect(actionSentence(me, "private")).toBe("A signed-in person can run it. It returns one number. It reads only their own rows.");
    expect(actionTitle(me.name)).toBe("Read admins for the signed-in person");
  });

  test("an actor checked only inside EXISTS says nothing about whose rows", () => {
    const list = query("SELECT id, body FROM messages WHERE EXISTS (SELECT 1 FROM admins WHERE user_id = ?1) ORDER BY id DESC LIMIT 100", [actor]);
    expect(actionSentence(list, "private")).toBe("A signed-in person can run it. The list stops at 100 rows.");
    expect(actionTitle("messages.list")).toBe("List messages");
  });

  test("other shapes that mention the actor claim nothing", () => {
    for (const statement of [
      "SELECT id FROM notes WHERE user_id = ?1 OR shared = 1 LIMIT 10",
      "SELECT id FROM notes WHERE NOT user_id = ?1 LIMIT 10",
      "SELECT id FROM notes WHERE user_id != ?1 LIMIT 10",
      "SELECT id FROM notes WHERE user_id <= ?1 LIMIT 10",
      "SELECT id FROM notes WHERE score + user_id = ?1 LIMIT 10",
      "SELECT id FROM notes WHERE (user_id = ?1 OR shared = 1) LIMIT 10",
      "SELECT id FROM notes WHERE CASE WHEN a AND user_id = ?1 AND b THEN 1 ELSE 1 END LIMIT 10",
      "SELECT id FROM notes WHERE user_id = ?2 LIMIT 10",
      "SELECT id FROM notes LIMIT 10",
      "SELECT m.body FROM memberships me JOIN messages m ON m.room = me.room WHERE me.user_id = ?1 LIMIT 100",
      "SELECT m.body FROM memberships me, messages m WHERE me.user_id = ?1 LIMIT 100",
      "SELECT (SELECT count(*) FROM messages) AS n, id FROM notes WHERE user_id = ?1 LIMIT 10",
      "UPDATE notes SET title = other.title FROM other WHERE other.user_id = ?1",
      "INSERT INTO notes (title) SELECT title FROM drafts WHERE owner = ?1",
      "INSERT INTO notes (id, title) VALUES (?2, 'x') ON CONFLICT (id) DO UPDATE SET title = 'y' WHERE owner = ?1",
      "WITH mine AS (SELECT id FROM notes) SELECT id FROM notes WHERE user_id = ?1 LIMIT 10",
    ]) expect(actorFiltered(statement, [actor, input]), statement).toBe(false);
  });

  test("a bare ? takes the next number, as SQLite counts it", () => {
    expect(actorFiltered("SELECT id FROM notes WHERE length(title) > ? AND user_id = ? LIMIT 5", [input, actor])).toBe(true);
    expect(actorFiltered("SELECT id FROM notes WHERE length(title) > ? AND user_id = ? LIMIT 5", [actor, input])).toBe(false);
    expect(actorFiltered("SELECT id FROM notes WHERE length(title) > ?2 AND user_id = ? LIMIT 5", [input, input, actor])).toBe(true);
    expect(actorFiltered('SELECT id FROM notes WHERE "user_id" = ?1 LIMIT 5', [actor])).toBe(true);
    expect(actorFiltered("SELECT id FROM notes WHERE ?1 = user_id LIMIT 5", [actor])).toBe(true);
  });

  test("a change scoped to the actor changes their own rows; one that writes the actor claims nothing", () => {
    const mutation = (statement: string, bindings: Array<{ source: string }>) => ({ name: "notes.update", kind: "mutation", access: "authenticated" as const, statement, bindings });
    expect(actionSentence(mutation("UPDATE notes SET title = ?1 WHERE id = ?2 AND user_id = ?3", [input, input, actor]), "private"))
      .toBe("A signed-in person can run it. It changes only their own rows.");
    expect(actionSentence(mutation("UPDATE notes SET user_id = ?1 WHERE id = ?2", [actor, input]), "private")).toBe("A signed-in person can run it.");
    expect(actionSentence(mutation("INSERT INTO notes (user_id, title) VALUES (?1, ?2)", [actor, input]), "private")).toBe("A signed-in person can run it.");
  });

  test("the Deploy dialog and Access read the same sentence from the parsed file", () => {
    const { actions } = deployReadiness(withActions({
      "notes.mine": { kind: "query", access: "authenticated", statement: "SELECT id FROM notes WHERE user_id = ?1 LIMIT 20", bindings: [{ source: "actor", field: "id" }] },
    }));
    expect(actionSentence(actions[0], "private")).toBe("A signed-in person can run it. The list stops at 20 rows. It reads only their own rows.");
  });
});
