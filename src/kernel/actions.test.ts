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

  test("rejects structural and multi-statement SQL", () => {
    expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "notes.drop": { kind: "mutation", access: "anonymous", statement: "DROP TABLE notes", bindings: [] },
    } }))).toThrow("only allows");
    expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "notes.list": { kind: "query", access: "anonymous", statement: "SELECT * FROM notes; DELETE FROM notes", bindings: [] },
    } }))).toThrow("exactly one");
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
    expect(() => params({ limit: "5" })).toThrow("finite number");
    expect(() => params({ done: 0 })).toThrow("boolean");
    expect(() => params({ other: null })).toThrow("Unexpected");
  });

  test("required inputs still reject omitted values and null", () => {
    const program = parseServerActionSource(source);
    for (const input of [{}, { body: null }, { body: "" }]) {
      expect(() => resolveServerAction(program, "notes.create", input, { actorId: "user_1" })).toThrow("non-empty");
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
