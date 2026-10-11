import { describe, expect, test } from "bun:test";
import { actionDeclarations, parseServerActionSource, resolveServerAction } from "./actions";

const source = JSON.stringify({
  version: 1,
  actions: {
    "notes.create": {
      kind: "mutation",
      access: "authenticated",
      statement: "INSERT INTO notes (owner_id, body, created_at) VALUES (?, ?, ?)",
      bindings: [
        { source: "actor", field: "id" },
        { source: "input", key: "body", type: "string", maxLength: 100 },
        { source: "kernel", field: "now" },
      ],
    },
  },
});

describe("server actions", () => {
  test("compiles declarations without exposing their SQL implementation", () => {
    const program = parseServerActionSource(source);
    expect(actionDeclarations(program)).toEqual([{ name: "notes.create", kind: "mutation", access: "authenticated" }]);
    expect(JSON.stringify(actionDeclarations(program))).not.toContain("INSERT");
  });

  test("resolves only declared typed bindings", () => {
    const program = parseServerActionSource(source);
    expect(resolveServerAction(program, "notes.create", { body: "private note" }, { actorId: "user_1", now: 42 }).params)
      .toEqual(["user_1", "private note", 42]);
    expect(() => resolveServerAction(program, "notes.create", { body: "note", ownerId: "attacker" }, { actorId: "user_1" }))
      .toThrow("Unexpected");
    expect(() => resolveServerAction(program, "notes.create", { body: "note" }, { actorId: null }))
      .toThrow("authenticated");
  });

  test("the actor role binds a person's role and null for a visitor or a key, as on the Kernel", () => {
    const program = parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "members.promote": { kind: "mutation", access: "authenticated", statement: "UPDATE members SET role = 'admin' WHERE id = ?1 AND ?2 = 'owner'",
        bindings: [{ source: "input", key: "id", type: "number" }, { source: "actor", field: "role" }], guard: "Only the owner can promote." },
      "me.owner": { kind: "query", access: "anonymous", statement: "SELECT ?1 = 'owner' AS owner LIMIT 1", bindings: [{ source: "actor", field: "role" }] },
    } }));
    expect(resolveServerAction(program, "members.promote", { id: 4 }, { actorId: "olga", actorRole: "owner" }).params).toEqual([4, "owner"]);
    expect(resolveServerAction(program, "members.promote", { id: 4 }, { actorId: "ana", actorRole: "member" }).params).toEqual([4, "member"]);
    expect(resolveServerAction(program, "members.promote", { id: 4 }, { actorId: "key-1" }).params).toEqual([4, null]);
    expect(resolveServerAction(program, "me.owner", {}, { actorId: null }).params).toEqual([null]);
    expect(() => resolveServerAction(program, "me.owner", { role: "owner" }, { actorId: null, actorRole: null })).toThrow("Unexpected action input: role");
    expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions: { "a.b": { kind: "query", access: "anonymous", statement: "SELECT ?1 AS x LIMIT 1", bindings: [{ source: "actor", field: "email" }] } } }))).toThrow("Invalid binding 0 in a.b.");
  });

  test("rejects structural and multi-statement SQL", () => {
    expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "notes.drop": { kind: "mutation", access: "anonymous", statement: "DROP TABLE notes", bindings: [] },
    } }))).toThrow("Tables and indexes belong in schema.sql");
    expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "notes.list": { kind: "query", access: "anonymous", statement: "SELECT * FROM notes; DELETE FROM notes", bindings: [] },
    } }))).toThrow("Action notes.list must contain one statement.");
  });

  test("nullable inputs bind omitted values and explicit null as SQL null", () => {
    const program = parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "notes.filter": {
        kind: "query", access: "anonymous", statement: "SELECT ?1, ?2, ?3",
        bindings: [
          { source: "input", key: "q", type: "string", maxLength: 4, nullable: true },
          { source: "input", key: "limit", type: "number", nullable: true },
          { source: "input", key: "done", type: "boolean", nullable: true },
        ],
      },
    } }));
    const params = (input?: unknown) => resolveServerAction(program, "notes.filter", input, { actorId: null }).params;
    expect(params()).toEqual([null, null, null]);
    expect(params({ q: null, limit: null, done: null })).toEqual([null, null, null]);
    expect(params({ q: "ship", limit: 0, done: false })).toEqual(["ship", 0, 0]);
    expect(params({ q: "ship", limit: 2.5, done: true })).toEqual(["ship", 2.5, 1]);
    expect(() => params({ q: "" })).toThrow("non-empty");
    expect(() => params({ q: "longer" })).toThrow("too long");
    expect(() => params({ q: 1 })).toThrow("non-empty");
    expect(() => params({ limit: "5" })).toThrow("limit must be a number.");
    expect(() => params({ done: 0 })).toThrow("boolean");
    expect(() => params({ other: null })).toThrow("Unexpected");
  });

  test("required inputs still reject omitted values and null, in the Kernel's words", () => {
    const program = parseServerActionSource(source);
    expect(() => resolveServerAction(program, "notes.create", {}, { actorId: "user_1" })).toThrow("Missing action input: body");
    for (const input of [{ body: null }, { body: "" }]) {
      expect(() => resolveServerAction(program, "notes.create", input, { actorId: "user_1" })).toThrow("body must be a non-empty string.");
    }
  });

  test("a query that reads rows needs a top-level LIMIT, and a total does not", () => {
    const program = (statement: string) => JSON.stringify({
      version: 1,
      actions: { "notes.list": { kind: "query", access: "anonymous", statement, bindings: [] } },
    });
    expect(() => parseServerActionSource(program("SELECT id FROM notes"))).toThrow("reads rows and has no LIMIT");
    expect(() => parseServerActionSource(program("SELECT id FROM notes WHERE title = 'LIMIT 1'"))).toThrow("no LIMIT");
    expect(() => parseServerActionSource(program("SELECT count(*) AS n FROM notes"))).not.toThrow();
    expect(parseServerActionSource(program("SELECT id FROM notes LIMIT 20")).actions["notes.list"].statement).toContain("LIMIT 20");
  });

  test("mutations reject RETURNING without rejecting quoted text or comments", () => {
    const parse = (statement: string) => parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "notes.create": { kind: "mutation", access: "anonymous", statement, bindings: [] },
    } }));
    for (const statement of [
      "INSERT INTO notes(body) VALUES ('note') RETURNING id",
      "UPDATE notes SET body='note' returning body",
      "DELETE FROM notes RETURNING *",
    ]) expect(() => parse(statement)).toThrow("cannot use RETURNING");
    for (const statement of [
      "INSERT INTO notes(body) VALUES ('RETURNING')",
      "UPDATE notes SET body='note' /* RETURNING id */",
      'UPDATE notes SET "RETURNING"=1',
      "UPDATE notes SET `RETURNING`=1",
      "UPDATE notes SET [RETURNING]=1",
      "UPDATE notes SET body=:returning",
      "UPDATE notes SET body=@returning",
      "UPDATE notes SET body=$returning",
    ]) expect(() => parse(statement)).not.toThrow();
  });

  test("nullable is a boolean option for input bindings only", () => {
    for (const binding of [
      { source: "input", key: "q", type: "string", nullable: "true" },
      { source: "input", key: "q", type: "string", nullable: null },
      { source: "actor", field: "id", nullable: true },
    ]) {
      expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions: {
        "notes.filter": { kind: "query", access: "anonymous", statement: "SELECT ?1", bindings: [binding] },
      } }))).toThrow("nullable");
    }
  });

  /** The Kernel's validate_action refuses "granted", so the gate must not wave it through. */
  test("rejects the granted access policy the Kernel cannot deploy", () => {
    expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "notes.list": { kind: "query", access: "granted", statement: "SELECT * FROM notes LIMIT 10", bindings: [] },
    } }))).toThrow('"access": "granted"');
  });
});

describe("action shape", () => {
  // kernel-rs/src/action_shape.rs runs the same cases against the Kernel's parser.
  test("refuses the same names, access, kinds, bindings and sizes as the Kernel, with the same messages", async () => {
    const cases: { case: string; actions: Record<string, unknown>; error: string | null }[] =
      await Bun.file(new URL("../../kernel-rs/src/action-cases.json", import.meta.url)).json();
    expect(cases.length).toBeGreaterThanOrEqual(25);
    for (const item of cases) {
      let message: string | null = null;
      try {
        parseServerActionSource(JSON.stringify({ version: 1, actions: item.actions }));
      } catch (error) {
        message = (error as Error).message;
      }
      expect(message, item.case).toBe(item.error);
    }
  });

  test("accepts nullable: false on a binding that is not an input, as the Kernel does", () => {
    expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "notes.mine": { kind: "query", access: "authenticated", statement: "SELECT id FROM notes WHERE user_id = ?1 LIMIT 5", bindings: [{ source: "actor", field: "id", nullable: false }] },
    } }))).not.toThrow();
  });
});

describe("http bindings", () => {
  // kernel-rs/src/http.rs runs the same cases against the Kernel's parser.
  test("refuse the same declarations as the Kernel, with the same messages", async () => {
    const cases: { case: string; actions: Record<string, unknown>; error: string | null }[] =
      await Bun.file(new URL("../../kernel-rs/src/http-binding-cases.json", import.meta.url)).json();
    expect(cases.length).toBeGreaterThan(15);
    for (const item of cases) {
      const parse = () => parseServerActionSource(JSON.stringify({ version: 1, actions: item.actions }));
      if (item.error === null) expect(parse, item.case).not.toThrow();
      else expect(parse, item.case).toThrow(item.error);
    }
  });

  test("keep the binding on the parsed action and out of the declarations", () => {
    const program = parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "orders.receive": {
        kind: "mutation", access: "anonymous", statement: "INSERT INTO orders (external_id) VALUES (?1)",
        bindings: [{ source: "input", key: "id", type: "string" }],
        http: { method: "POST", path: "/hooks/orders", signature: { header: "X-Signature", secret: "orders_webhook" } },
      },
    } }));
    expect(program.actions["orders.receive"].http).toEqual({ method: "POST", path: "/hooks/orders", signature: { header: "X-Signature", secret: "orders_webhook" } });
    expect(actionDeclarations(program)).toEqual([{ name: "orders.receive", kind: "mutation", access: "anonymous" }]);
  });
});

describe("signals", () => {
  // kernel-rs/src/usage.rs runs the same cases against the Kernel's parser.
  test("refuse the same declarations as the Kernel, with the same messages", async () => {
    const cases: { case: string; actions: Record<string, unknown>; error: string | null }[] =
      await Bun.file(new URL("../../kernel-rs/src/signal-cases.json", import.meta.url)).json();
    expect(cases.length).toBeGreaterThan(15);
    for (const item of cases) {
      const parse = () => parseServerActionSource(JSON.stringify({ version: 1, actions: item.actions }));
      if (item.error === null) expect(parse, item.case).not.toThrow();
      else expect(parse, item.case).toThrow(item.error);
    }
  });

  test("keep the signal on the parsed action and declare a signal action as its own kind", () => {
    const program = parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "newsletter.join": { kind: "signal", access: "anonymous", bindings: [{ source: "input", key: "source", type: "string", maxLength: 40 }],
        signal: { name: "newsletter.joined", properties: ["source"] } },
    } }));
    expect(program.actions["newsletter.join"]).toMatchObject({ kind: "signal", statement: "", signal: { name: "newsletter.joined", properties: ["source"] } });
    expect(actionDeclarations(program)).toEqual([{ name: "newsletter.join", kind: "signal", access: "anonymous" }]);
    const resolved = resolveServerAction(program, "newsletter.join", { source: "footer" }, { actorId: null });
    expect(resolved.params).toEqual(["footer"]);
    expect(() => resolveServerAction(program, "newsletter.join", { source: "footer", extra: 1 }, { actorId: null })).toThrow("Unexpected action input: extra");
  });

  test("count a signal action toward the action cap", () => {
    const actions: Record<string, unknown> = {};
    for (let index = 0; index < 100; index += 1) {
      actions[`items.read${String.fromCharCode(97 + (index % 26))}${String.fromCharCode(97 + Math.floor(index / 26))}`] =
        { kind: "query", access: "anonymous", statement: "SELECT 1 AS n", bindings: [] };
    }
    expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions }))).not.toThrow();
    actions["page.like"] = { kind: "signal", access: "anonymous", bindings: [], signal: { name: "page.liked" } };
    expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions }))).toThrow("A project may declare at most 100 actions.");
  });
});

describe("schedules", () => {
  // kernel-rs/src/schedule.rs runs the same cases against the Kernel's parser.
  test("refuse the same declarations as the Kernel, with the same messages", async () => {
    const cases: { case: string; actions: Record<string, unknown>; error: string | null }[] =
      await Bun.file(new URL("../../kernel-rs/src/schedule-cases.json", import.meta.url)).json();
    expect(cases.length).toBeGreaterThanOrEqual(20);
    for (const item of cases) {
      let message: string | null = null;
      try {
        parseServerActionSource(JSON.stringify({ version: 1, actions: item.actions }));
      } catch (error) {
        message = (error as Error).message;
      }
      expect(message, item.case).toBe(item.error);
    }
  });

  test("keep the schedule on the parsed mutation", () => {
    const program = parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "holds.release": { kind: "mutation", access: "authenticated", schedule: { daily: "02:00" },
        statement: "UPDATE holds SET released_at = ?1 WHERE until < ?1", bindings: [{ source: "kernel", field: "now" }] },
    } }));
    expect(program.actions["holds.release"].schedule).toEqual({ daily: "02:00" });
  });
});

describe("guards", () => {
  // kernel-rs/src/guard.rs runs the same cases against the Kernel's parser.
  test("refuse the same declarations as the Kernel, with the same messages", async () => {
    const cases: { case: string; actions: Record<string, unknown>; error: string | null }[] =
      await Bun.file(new URL("../../kernel-rs/src/guard-cases.json", import.meta.url)).json();
    expect(cases.length).toBeGreaterThanOrEqual(8);
    for (const item of cases) {
      let message: string | null = null;
      try {
        parseServerActionSource(JSON.stringify({ version: 1, actions: item.actions }));
      } catch (error) {
        message = (error as Error).message;
      }
      expect(message, item.case).toBe(item.error);
    }
  });

  test("keep the guard on the parsed mutation", () => {
    const program = parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "members.promote": { kind: "mutation", access: "authenticated", guard: "Only an organizer can promote a member.",
        statement: "UPDATE members SET role = 'organizer' WHERE id = ?1", bindings: [{ source: "actor", field: "id" }] },
    } }));
    expect(program.actions["members.promote"].guard).toBe("Only an organizer can promote a member.");
  });
});

describe("notify", () => {
  // kernel-rs/src/notify.rs runs the same cases against the Kernel's parser.
  test("refuse the same declarations as the Kernel, with the same messages", async () => {
    const cases: { case: string; actions: Record<string, unknown>; error: string | null }[] =
      await Bun.file(new URL("../../kernel-rs/src/notify-cases.json", import.meta.url)).json();
    expect(cases.length).toBeGreaterThanOrEqual(15);
    for (const item of cases) {
      let message: string | null = null;
      try {
        parseServerActionSource(JSON.stringify({ version: 1, actions: item.actions }));
      } catch (error) {
        message = (error as Error).message;
      }
      expect(message, item.case).toBe(item.error);
    }
  });

  test("keep the notice on the parsed mutation", () => {
    const notify = { to: "actor", subject: "Booked {input.room}", body: "You booked {input.room}." };
    const program = parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "bookings.create": { kind: "mutation", access: "authenticated", notify,
        statement: "INSERT INTO bookings (user_id, room_id) VALUES (?1, ?2)",
        bindings: [{ source: "actor", field: "id" }, { source: "input", key: "room", type: "string" }] },
    } }));
    expect(program.actions["bookings.create"].notify).toEqual(notify as never);
  });
});
