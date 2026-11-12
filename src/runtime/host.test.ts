import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cloneStarter } from "../lib/starter";
import { handlePreviewMcp } from "../mcp/preview";
import { sdkContract } from "../sdk/contract";
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

  test("the preview has no Google sign-in and says so", async () => {
    const host = new MiniAppRuntimeHost("google", schema, actions);
    await expect(host.handle("auth.signInWithGoogle", {})).rejects.toThrow("The editor preview has no Google sign-in.");
    expect(await host.handle("auth.session", {})).toMatchObject({ role: "anonymous" });
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

  test("a guarded mutation that changes no row is refused with its guard, and a route-only action is refused", async () => {
    const guarded = JSON.stringify({
      version: 1,
      actions: {
        "slots.take": {
          kind: "mutation", access: "anonymous", guard: "That slot is taken.",
          statement: "INSERT INTO slots (slot) SELECT ?1 WHERE NOT EXISTS (SELECT 1 FROM slots WHERE slot = ?1)",
          bindings: [{ source: "input", key: "slot", type: "number" }],
        },
        "slots.hook": {
          kind: "mutation", access: "anonymous", statement: "INSERT INTO slots (slot) VALUES (?1)",
          bindings: [{ source: "input", key: "slot", type: "number" }],
          http: { method: "POST", path: "/hooks/slots", signature: { header: "X-Signature", secret: "slots_hook" } },
        },
      },
    });
    const host = new MiniAppRuntimeHost("guard", "CREATE TABLE IF NOT EXISTS slots (slot INTEGER NOT NULL);", guarded);
    expect(await host.handle("actions.invoke", { name: "slots.take", input: { slot: 9 } })).toEqual({ changes: 1, lastInsertId: 1 });
    await expect(host.handle("actions.invoke", { name: "slots.take", input: { slot: 9 } })).rejects.toThrow("That slot is taken.");
    await expect(host.handle("actions.invoke", { name: "slots.hook", input: { slot: 10 } }))
      .rejects.toThrow("Action slots.hook answers only on its HTTP route, POST /hooks/slots.");
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

  test("an upload with an unknown visibility is refused instead of stored as private", async () => {
    const host = new MiniAppRuntimeHost("files-visibility", schema, actions);
    host.setPreviewActor("alice");
    const payload = { name: "note.txt", type: "text/plain", size: 2, dataUrl: "data:text/plain,hi" };
    for (const visibility of ["organizers", "Public", 1]) {
      await expect(host.handle("files.upload", { ...payload, visibility })).rejects.toThrow("File visibility must be private or public.");
    }
    expect(host.getFiles()).toEqual([]);
  });

  test("app-user file policy applies to existing files and rejects conflicting visibility", async () => {
    const host = new MiniAppRuntimeHost("files-app-users", schema, actions);
    const payload = { name: "note.txt", type: "text/plain", size: 2, dataUrl: "data:text/plain,hi" };
    host.setPreviewActor("alice");
    const existing = await host.handle("files.upload", payload) as { id: string };
    const formerlyPublic = await host.handle("files.upload", { ...payload, visibility: "public" }) as { id: string };
    host.setFileAccess("app-users");
    const shared = await host.handle("files.upload", payload) as { id: string; visibility: string };
    expect(shared.visibility).toBe("app-users");
    await expect(host.handle("files.upload", { ...payload, visibility: "public" })).rejects.toThrow("File visibility is fixed to app-users by this app.");
    await expect(host.handle("files.update", { fileId: existing.id, visibility: "private" })).rejects.toThrow("File visibility is fixed to app-users by this app.");
    expect(await host.handle("files.update", { fileId: formerlyPublic.id, visibility: "app-users" })).toMatchObject({ visibility: "app-users" });
    host.setPreviewActor("bob");
    await expect(host.handle("files.update", { fileId: existing.id, visibility: "private" })).rejects.toThrow("Only the person who uploaded this file may change or delete it.");
    expect(await host.handle("files.url", { fileId: existing.id })).toBe(payload.dataUrl);
    expect(await host.handle("files.list")).toEqual(expect.arrayContaining([expect.objectContaining({ id: existing.id, visibility: "app-users" })]));
    host.setPreviewActor("guest");
    expect(await host.handle("files.list")).toEqual([]);
    await expect(host.handle("files.url", { fileId: shared.id })).rejects.toThrow("File not found.");
    host.setFileAccess(undefined);
    expect(await host.handle("files.url", { fileId: formerlyPublic.id })).toBe(payload.dataUrl);
  });

  test("the uploader publishes, hides and deletes a preview file, and files.url answers its data URL", async () => {
    const host = new MiniAppRuntimeHost("files-lifecycle", schema, actions);
    const payload = { name: "note.txt", type: "text/plain", size: 2, dataUrl: "data:text/plain,hi" };
    host.setPreviewActor("alice");
    const draft = await host.handle("files.upload", payload) as { id: string; ownerId: string };
    expect(await host.handle("files.url", { fileId: draft.id })).toBe("data:text/plain,hi");
    await expect(host.handle("files.url", {})).rejects.toThrow("A file ID is required.");
    await expect(host.handle("files.update", { fileId: draft.id, visibility: "organizers" })).rejects.toThrow("File visibility must be private or public.");

    host.setPreviewActor("bob");
    await expect(host.handle("files.url", { fileId: draft.id })).rejects.toThrow("File not found.");
    await expect(host.handle("files.update", { fileId: draft.id, visibility: "public" })).rejects.toThrow("File not found.");
    await expect(host.handle("files.delete", { fileId: draft.id })).rejects.toThrow("File not found.");

    host.setPreviewActor("alice");
    const published = await host.handle("files.update", { fileId: draft.id, visibility: "public" }) as Record<string, unknown>;
    expect(published).toMatchObject({ id: draft.id, visibility: "public", ownerId: draft.ownerId });
    expect(published.url).toBeUndefined();
    host.setPreviewActor("guest");
    expect(await host.handle("files.url", { fileId: draft.id })).toBe("data:text/plain,hi");
    await expect(host.handle("files.delete", { fileId: draft.id })).rejects.toThrow("authentication-required");
    host.setPreviewActor("bob");
    await expect(host.handle("files.delete", { fileId: draft.id })).rejects.toThrow("Only the person who uploaded this file may change or delete it.");
    await expect(host.handle("files.update", { fileId: draft.id, visibility: "private" })).rejects.toThrow("Only the person who uploaded this file may change or delete it.");

    host.setPreviewActor("alice");
    expect(await host.handle("files.delete", { fileId: draft.id })).toEqual({ id: draft.id, deleted: true });
    expect(host.getFiles()).toEqual([]);
    await expect(host.handle("files.url", { fileId: draft.id })).rejects.toThrow("File not found.");
    await expect(host.handle("files.delete", { fileId: draft.id })).rejects.toThrow("File not found.");
  });
});

describe("the preview refuses miniapp.actions.call with the Kernel's words", () => {
  const callErrors = sdkContract.modules.find((module) => module.name === "actions")!.calls.find((call) => call.name === "call")!.errors;
  /** The contract's message for a refusal the preview shares with the Kernel, with its placeholders filled. */
  function kernelMessage(template: string, fill: Record<string, string> = {}): string {
    const error = callErrors.find((candidate) => candidate.message === template);
    expect(error, template).toBeDefined();
    // Absent means the preview answers with the same text as the Kernel.
    expect(error!.preview, template).toBeUndefined();
    return template.replace(/<(key|name)>/g, (_, slot: string) => fill[slot] ?? "");
  }

  const schema = "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, title TEXT, qty REAL, done INTEGER, owner TEXT);";
  const program = JSON.stringify({ version: 1, actions: {
    "items.add": { kind: "mutation", access: "anonymous", statement: "INSERT INTO items (title, qty, done) VALUES (?1, ?2, ?3)", bindings: [
      { source: "input", key: "title", type: "string", maxLength: 5 }, { source: "input", key: "qty", type: "number" }, { source: "input", key: "done", type: "boolean" }] },
    "items.mine": { kind: "query", access: "anonymous", statement: "SELECT id FROM items WHERE owner = ?1 LIMIT 5", bindings: [{ source: "actor", field: "id" }] },
    "items.private": { kind: "query", access: "authenticated", statement: "SELECT id FROM items LIMIT 5", bindings: [] },
    "items.feed": { kind: "query", access: "anonymous", statement: "SELECT id FROM items LIMIT 5", bindings: [], http: { method: "GET", path: "/api/feed", callers: ["key"] } },
    "items.tidy": { kind: "mutation", access: "anonymous", schedule: { every: "15m" }, statement: "DELETE FROM items WHERE qty < 0", bindings: [] },
    "items.many": { kind: "query", access: "anonymous", statement: "SELECT d FROM (WITH RECURSIVE n(d) AS (SELECT 1 UNION ALL SELECT d + 1 FROM n WHERE d < 1500) SELECT d FROM n) LIMIT 1500", bindings: [] },
  } });

  test.each([
    ["an undeclared action", { name: "items.nope" }, ["Server action not declared."]],
    ["a name that is not a string", { name: 5 }, ["Expected a string."]],
    ["input that is null", { name: "items.add", input: null }, ["Arguments must be an object."]],
    ["an input no binding declares", { name: "items.many", input: { extra: 1 } }, ["Unexpected action input: <key>", { key: "extra" }]],
    ["an authenticated action and no one signed in", { name: "items.private" }, ["authentication-required"]],
    ["an anonymous action that binds the actor and no one signed in", { name: "items.mine" }, ["This action requires an authenticated actor."]],
    ["an action whose route is its only door", { name: "items.feed" }, ["Action <name> answers only on its HTTP route, <key>.", { name: "items.feed", key: "GET /api/feed" }]],
    ["a scheduled mutation with no route", { name: "items.tidy" }, ["Action <name> runs only on its schedule.", { name: "items.tidy" }]],
    ["a required input left out", { name: "items.add", input: { qty: 1, done: true } }, ["Missing action input: <key>", { key: "title" }]],
    ["an empty string", { name: "items.add", input: { title: " ", qty: 1, done: true } }, ["<key> must be a non-empty string.", { key: "title" }]],
    ["a string past its maxLength", { name: "items.add", input: { title: "too long", qty: 1, done: true } }, ["<key> is too long.", { key: "title" }]],
    ["a number sent as a string", { name: "items.add", input: { title: "a", qty: "1", done: true } }, ["<key> must be a number.", { key: "qty" }]],
    ["a boolean sent as a number", { name: "items.add", input: { title: "a", qty: 1, done: 1 } }, ["<key> must be a boolean.", { key: "done" }]],
    ["a result past 1000 rows", { name: "items.many" }, ["Query result exceeds 2 MiB or 1000 rows."]],
  ] as const)("%s", async (_, args, [template, fill]) => {
    const host = new MiniAppRuntimeHost("parity", schema, program);
    host.setPreviewActor("guest");
    const answer = await handlePreviewMcp(host, {
      jsonrpc: "2.0", id: 1, method: "tools/call",
      params: { name: "actions_invoke", arguments: args, _meta: { "konae/capability": "cap" } },
    }, "cap");
    expect(answer).toMatchObject({ result: { isError: true, content: [{ text: kernelMessage(template, fill as Record<string, string> | undefined) }] } });
  });
});

describe("the preview refuses miniapp.files.upload with the Kernel's words", () => {
  const uploadErrors = sdkContract.modules.find((module) => module.name === "files")!.calls.find((call) => call.name === "upload")!.errors;

  test.each([
    ["an empty file", 0],
    ["a file over 2 MiB", 2 * 1024 * 1024 + 1],
  ])("%s", async (_, size) => {
    const message = "Files must be between 1 byte and 2 MiB.";
    expect(uploadErrors.find((error) => error.message === "Files must be between 1 byte and <limit>.")?.preview).toBeUndefined();
    const host = new MiniAppRuntimeHost("upload", schema, actions);
    host.setPreviewActor("alice");
    const answer = await handlePreviewMcp(host, {
      jsonrpc: "2.0", id: 1, method: "tools/call",
      params: { name: "preview_files_upload", arguments: { name: "a.txt", type: "text/plain", size, dataUrl: "data:text/plain;base64," }, _meta: { "konae/capability": "cap" } },
    }, "cap");
    expect(answer).toMatchObject({ result: { isError: true, content: [{ text: message }] } });
    expect(host.getFiles()).toEqual([]);
  });
});

