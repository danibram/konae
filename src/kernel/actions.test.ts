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
      "notes.drop": { kind: "mutation", access: "granted", statement: "DROP TABLE notes", bindings: [] },
    } }))).toThrow("only allows");
    expect(() => parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "notes.list": { kind: "query", access: "anonymous", statement: "SELECT * FROM notes; DELETE FROM notes", bindings: [] },
    } }))).toThrow("exactly one");
  });
});
