import { describe, expect, test } from "bun:test";
import initSqlJs from "sql.js";
import { createCapsuleValidator } from "../agent/gate";
import { parseServerActionSource, resolveServerAction } from "./actions";
import { checkConnects, thenOf } from "./calls";
import { expectedOutcome, outcomeOf, type Outcome } from "./refusal-cases";
import { normalizeManifest } from "../deployments/manifest";

type Case = { case: string; actions?: Record<string, unknown>; connects?: unknown; error: string | null; code?: string };
const cases: Case[] = await Bun.file(new URL("../../kernel-rs/kernel/src/call-cases.json", import.meta.url)).json();
const schema = "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, notice TEXT, owner TEXT);";

/** The answer one door gives a case: null when it passes, else its code and message. */
function editorAnswer(item: Case): Outcome {
  return outcomeOf(() => "connects" in item ? checkConnects(item.connects) : parseServerActionSource(JSON.stringify({ version: 1, actions: item.actions })));
}

describe("calls between apps at the editor's door", () => {
  // kernel-rs/kernel/src/calls.rs runs the same file.
  test("every case in kernel-rs/kernel/src/call-cases.json gets the Kernel's answer", () => {
    expect(cases.length).toBeGreaterThanOrEqual(40);
    for (const item of cases) expect(editorAnswer(item), item.case).toEqual(expectedOutcome(item));
  });

  test("the Validation Gate refuses every action case with the Kernel's words", async () => {
    const validator = createCapsuleValidator(() => initSqlJs());
    for (const item of cases.filter((candidate) => candidate.actions)) {
      const result = await validator.validate({
        "index.html": "<!doctype html><title>t</title>", "schema.sql": schema,
        "actions.json": JSON.stringify({ version: 1, actions: item.actions }),
      } as never);
      expect(result.errors.map((error) => ({ code: error.code, message: error.detail })), item.case).toEqual(item.error === null ? [] : [expectedOutcome(item)]);
    }
  });

  test("a call keeps its declaration, resolves its inputs and names its then", () => {
    const program = parseServerActionSource(JSON.stringify({ version: 1, actions: {
      "inbox.tell": {
        kind: "call", access: "authenticated", method: "POST", url: "https://inbox.example.org/hooks/events",
        bindings: [{ source: "input", key: "title", type: "string", maxLength: 20 }, { source: "input", key: "taskId", type: "number" }],
        body: { title: "title", id: "taskId" }, auth: { secret: "inbox_events" },
        then: { action: "tasks.noted", inputs: { id: "/input/taskId", notice: "/body/error" } },
      },
      "tasks.noted": {
        kind: "mutation", access: "authenticated", statement: "UPDATE tasks SET notice = ?2 WHERE id = ?1",
        bindings: [{ source: "input", key: "id", type: "number" }, { source: "input", key: "notice", type: "string", nullable: true }],
      },
    } }));
    expect(program.actions["inbox.tell"].call).toEqual({
      url: "https://inbox.example.org/hooks/events", method: "POST", body: { id: "taskId", title: "title" },
      auth: { secret: "inbox_events", header: "X-Konae-Signature" },
      then: { action: "tasks.noted", inputs: { id: "/input/taskId", notice: "/body/error" } },
    });
    expect(resolveServerAction(program, "inbox.tell", { title: "Down", taskId: 3 }, { actorId: "u" }).params).toEqual(["Down", 3]);
    expect(() => resolveServerAction(program, "inbox.tell", { title: "x".repeat(21), taskId: 3 }, { actorId: "u" })).toThrow("title is too long.");
    expect(thenOf(program.actions, "tasks.noted")).toBe("inbox.tell");
    expect(thenOf(program.actions, "inbox.tell")).toBeNull();
  });

  test("the manifest keeps connects and refuses what the Kernel refuses", () => {
    expect(normalizeManifest({ version: 1, visibility: "public", connects: ["https://api.example.com"] }).connects).toEqual(["https://api.example.com"]);
    expect(normalizeManifest({ version: 1, visibility: "public" }).connects).toBeUndefined();
    expect(() => normalizeManifest({ version: 1, visibility: "public", connects: ["http://api.example.com"] }))
      .toThrow("connects lists http://api.example.com, which is not an origin.");
  });
});
