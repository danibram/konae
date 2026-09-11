import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cloneStarter } from "../lib/starter";
import { MiniAppRuntimeHost } from "./host";

const localValues = new Map<string, string>();
const previousLocalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
let quotaBytes = Infinity;

beforeEach(() => {
  localValues.clear();
  quotaBytes = Infinity;
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => localValues.get(key) ?? null,
      setItem: (key: string, value: string) => {
        if (value.length > quotaBytes) throw new Error("QuotaExceededError");
        localValues.set(key, value);
      },
      removeItem: (key: string) => { localValues.delete(key); },
    },
  });
});

afterEach(() => {
  if (previousLocalStorage) Object.defineProperty(globalThis, "localStorage", previousLocalStorage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

const starter = cloneStarter();
const schema = starter.files["schema.sql"];
const actions = starter.files["actions.json"];

describe("runtime host", () => {
  /**
   * The bug this pins: the host is built during render and parsed actions.json
   * in its constructor, so one bad edit threw on every render and the editor
   * could not open the project again, even after a reload.
   */
  test("an invalid actions.json is reported, not thrown, and the last valid program keeps running", async () => {
    expect(() => new MiniAppRuntimeHost("broken", schema, "{ \"version\": 1, ")).not.toThrow();
    const broken = new MiniAppRuntimeHost("broken", schema, "{ \"version\": 1, ");
    expect(broken.actionsError).toContain("valid JSON");
    await expect(broken.handle("actions.invoke", { name: "tasks.list" })).rejects.toThrow("actions.json is invalid");
    // Nothing valid ever loaded, so there is no last valid program to speak of.
    expect(broken.previewErrors()[0].message).toContain("has no server actions until it is fixed");
    expect(broken.previewErrors()[0].message).not.toContain("last valid");

    const host = new MiniAppRuntimeHost("kept", schema, actions);
    const seen: Array<string | null> = [];
    host.subscribeDiagnostics((diagnostics) => seen.push(diagnostics.actionsError));
    host.setActions("not json");
    expect(host.actionsError).toContain("valid JSON");
    expect(host.previewErrors()[0]).toMatchObject({ kind: "actions" });
    expect(host.previewErrors()[0].message).toContain("still runs the last valid one");
    // A reconnect clears runtime failures; it must not hide the broken file.
    host.clearPreviewErrors();
    expect(host.previewErrors()[0]).toMatchObject({ kind: "actions" });
    expect(await host.handle("actions.invoke", { name: "tasks.list" })).toEqual([]);

    host.setActions(actions);
    expect(host.actionsError).toBeNull();
    expect(host.previewErrors()).toEqual([]);
    expect(seen).toEqual([null, expect.stringContaining("valid JSON"), null]);
  });

  /**
   * The bug this pins: setSchema recorded the text before running it, so a
   * failure was an unhandled rejection and running the same text again later
   * was skipped as already applied.
   */
  test("a failing schema.sql is surfaced and the same text is retried later", async () => {
    const host = new MiniAppRuntimeHost("schema", "CREATE TABLE broken (", actions);
    await host.setSchema("CREATE TABLE broken (");
    expect(host.schemaError).toContain("schema.sql failed");
    expect(host.previewErrors()[0]).toMatchObject({ kind: "schema" });

    await host.setSchema(schema);
    expect(host.schemaError).toBeNull();
    expect(await host.handle("actions.invoke", { name: "tasks.list" })).toEqual([]);

    await host.setSchema("CREATE TABLE IF NOT EXISTS later (id INTEGER PRIMARY KEY);\nCREATE TABLE tasks (id INTEGER);");
    expect(host.schemaError).toContain("already exists");
    await host.setSchema(schema);
    expect(host.schemaError).toBeNull();
  });

  /**
   * The bug this pins: a write was released and then persisted, so a full
   * localStorage reported the write as failed while it stayed in memory and
   * showed up in the next read.
   */
  test("a write that cannot be saved is not left in memory", async () => {
    const host = new MiniAppRuntimeHost("quota", schema, actions);
    await host.handle("actions.invoke", { name: "tasks.create", input: { title: "kept" } });
    quotaBytes = 0;
    await expect(host.handle("actions.invoke", { name: "tasks.create", input: { title: "ghost" } })).rejects.toThrow("Quota");
    quotaBytes = Infinity;
    const rows = await host.handle("actions.invoke", { name: "tasks.list" }) as Array<{ title: string }>;
    expect(rows.map((row) => row.title)).toEqual(["kept"]);
  });

  test("a signed-out preview session has the Kernel's anonymous shape", async () => {
    const host = new MiniAppRuntimeHost("session", schema, actions);
    expect(await host.handle("auth.session", {})).toEqual({ userId: "anonymous", name: "Anonymous", role: "anonymous" });
    expect(host.getPreviewActor()).toBe("guest");
    expect(await host.handle("auth.signIn", {})).toEqual({ userId: "alice_session", name: "Alice", role: "member" });
  });

  test("a signed-out preview runs anonymous actions and refuses the rest, like a public deployment", async () => {
    const accessActions = JSON.stringify({
      version: 1,
      actions: {
        "notes.count": { kind: "query", access: "anonymous", statement: "SELECT COUNT(*) AS total FROM notes", bindings: [] },
        "notes.create": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes (owner) VALUES (?)", bindings: [{ source: "actor", field: "id" }] },
      },
    });
    const host = new MiniAppRuntimeHost("access", "CREATE TABLE IF NOT EXISTS notes (owner TEXT NOT NULL);", accessActions);
    expect(await host.handle("actions.invoke", { name: "notes.count" })).toEqual([{ total: 0 }]);
    await expect(host.handle("actions.invoke", { name: "notes.create" })).rejects.toThrow("authentication-required");
    host.setPreviewActor("bob");
    await host.handle("actions.invoke", { name: "notes.create" });
    host.setPreviewActor("guest");
    expect(await host.handle("actions.invoke", { name: "notes.count" })).toEqual([{ total: 1 }]);
  });

  /**
   * The Kernel binds an integer-valued number as INTEGER and measures maxLength
   * in UTF-16 code units, which is what JavaScript's string length counts.
   * sql.js binds integers in the 32-bit range as INTEGER, so these agree.
   * Larger integers, such as a millisecond timestamp, sql.js can only bind as
   * REAL; a column with INTEGER affinity stores them as integers anyway, which
   * is why the templates declare timestamp columns INTEGER.
   */
  test("binds numbers and measures strings the way the Kernel does", async () => {
    const bindingSchema = "CREATE TABLE IF NOT EXISTS probe (n_int INTEGER, n_text TEXT, n_any, ts INTEGER, label TEXT);";
    const bindingActions = JSON.stringify({
      version: 1,
      actions: {
        "probe.create": {
          kind: "mutation",
          access: "anonymous",
          statement: "INSERT INTO probe (n_int, n_text, n_any, ts, label) VALUES (?, ?, ?, ?, ?)",
          bindings: [
            { source: "input", key: "n", type: "number" },
            { source: "input", key: "n", type: "number" },
            { source: "input", key: "n", type: "number" },
            { source: "kernel", field: "now" },
            { source: "input", key: "label", type: "string", maxLength: 2 },
          ],
        },
        "probe.list": {
          kind: "query",
          access: "anonymous",
          statement: "SELECT typeof(n_int) AS a, typeof(n_text) AS b, n_text AS text, typeof(n_any) AS c, typeof(ts) AS d, label FROM probe LIMIT 5",
          bindings: [],
        },
      },
    });
    const host = new MiniAppRuntimeHost("binding", bindingSchema, bindingActions);
    // One astral character is two UTF-16 code units, so it fills a maxLength of 2.
    await host.handle("actions.invoke", { name: "probe.create", input: { n: 5, label: "\u{1F600}" } });
    await expect(host.handle("actions.invoke", { name: "probe.create", input: { n: 5, label: "\u{1F600}a" } })).rejects.toThrow("too long");
    expect(await host.handle("actions.invoke", { name: "probe.list" })).toEqual([
      { a: "integer", b: "text", text: "5", c: "integer", d: "integer", label: "\u{1F600}" },
    ]);
  });

  test("preview files stay with the uploader unless the upload is public", async () => {
    const host = new MiniAppRuntimeHost("files", schema, actions);
    const payload = { name: "note.txt", type: "text/plain", size: 2, dataUrl: "data:text/plain,hi" };
    await expect(host.handle("files.upload", payload)).rejects.toThrow("authentication-required");
    host.setPreviewActor("alice");
    const privateFile = await host.handle("files.upload", payload) as { id: string; visibility: string; ownerId: string };
    expect(privateFile.visibility).toBe("private");
    const publicFile = await host.handle("files.upload", { ...payload, visibility: "public" }) as { id: string; visibility: string };
    expect(publicFile.visibility).toBe("public");
    host.setPreviewActor("bob");
    const bobList = await host.handle("files.list") as Array<{ id: string; ownerId?: string }>;
    expect(bobList.map((file) => file.id)).toEqual([publicFile.id]);
    expect(bobList[0].ownerId).toBeUndefined();
    host.setPreviewActor("guest");
    expect(await host.handle("files.list")).toEqual([expect.objectContaining({ id: publicFile.id, visibility: "public" })]);
    host.setPreviewActor("alice");
    const aliceList = await host.handle("files.list") as Array<{ id: string }>;
    expect(aliceList.map((file) => file.id)).toEqual([publicFile.id, privateFile.id]);
  });
});
