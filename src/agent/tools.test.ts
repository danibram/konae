import { describe, expect, test } from "bun:test";
import wasmCases from "../../kernel-rs/kernel/src/functions-wasm-cases.json";
import initSqlJs, { type SqlJsStatic } from "sql.js";
import { cloneStarter, starterProject } from "../lib/starter";
import { applyTemplate, templateChoices } from "../lib/templates";
import { createCapsuleWorkspace } from "./capsule-env";
import type { AgentTool, CapsuleFiles, CapsuleValidator, CapsuleWorkspace } from "./contracts";
import { createCapsuleValidator } from "./gate";
import { createCapsuleTools, formatValidation, readLiveDatabase, readReleaseFacts } from "./tools";
import { refusal } from "../kernel/refusal";

let sql: Promise<SqlJsStatic> | undefined;
const loadSql = () => (sql ??= initSqlJs());

function setup(overrides: Partial<CapsuleFiles> = {}) {
  const workspace = createCapsuleWorkspace({ ...starterProject.files, ...overrides });
  const tools = createCapsuleTools(workspace, createCapsuleValidator(loadSql), { releaseFacts: async () => ({ modules: { files: { on: true } } }) });
  return { workspace, tools };
}

function tool(tools: AgentTool<any>[], name: string): AgentTool<any> {
  const found = tools.find((candidate) => candidate.name === name);
  if (!found) throw new Error(`No tool named ${name}`);
  return found;
}

function call(tools: AgentTool<any>[], name: string, params: unknown, signal?: AbortSignal) {
  return tool(tools, name).execute(`call_${name}`, params as never, signal);
}

function textOf(result: { content: Array<{ type: string }> }): string {
  return result.content.map((block) => (block.type === "text" ? (block as { text: string }).text : "")).join("");
}

describe("capsule tool set", () => {
  test("offers file edits, validation and candidate browser checks", () => {
    const { tools } = setup();
    expect(tools.map((candidate) => candidate.name)).toEqual([
      "read",
      "edit",
      "edit_json",
      "write",
      "validate_capsule",
      "preview_test",
      "preview_errors",
    ]);
    expect(tools.every((candidate) => candidate.label.length > 0)).toBe(true);
  });

  test("preview_errors reports what the running preview threw, and says so when it threw nothing", async () => {
    const quiet = createCapsuleTools(createCapsuleWorkspace(starterProject.files), createCapsuleValidator(loadSql));
    expect(textOf(await call(quiet, "preview_errors", {}))).toContain("unavailable");

    const failing = createCapsuleTools(createCapsuleWorkspace(starterProject.files), createCapsuleValidator(loadSql), {
      previewErrors: () => [
        { kind: "error", message: "Cannot read properties of null (reading 'addEventListener')", stack: "at app.js:49", count: 3 },
      ],
    });
    const reported = textOf(await call(failing, "preview_errors", {}));
    expect(reported).toContain("addEventListener");
    expect(reported).toContain("x3");
    expect(reported).toContain("app.js:49");
  });

  test("tells the model to validate after a schema or actions change", () => {
    const { tools } = setup();
    for (const name of ["edit", "write"]) {
      expect(tool(tools, name).description).toContain("validate_capsule");
      expect(tool(tools, name).description).toContain("/workspace/schema.sql");
    }
    expect(tool(tools, "write").description).toContain("/workspace/actions.json");
  });
});

describe("read", () => {
  test("returns whole files and line ranges", async () => {
    const { tools } = setup({ "app.js": "one\ntwo\nthree\nfour" });
    expect(textOf(await call(tools, "read", { path: "/workspace/app.js" }))).toBe("one\ntwo\nthree\nfour");

    const ranged = await call(tools, "read", { path: "app.js", offset: 2, limit: 2 });
    expect(textOf(ranged)).toBe("app.js lines 2-3 of 4\ntwo\nthree");
    expect(ranged.details).toMatchObject({ path: "/workspace/app.js", startLine: 2, endLine: 3, totalLines: 4 });
  });

  test("rejects paths that do not exist yet", async () => {
    const { tools } = setup();
    await expect(call(tools, "read", { path: "/workspace/package.json" })).rejects.toThrow("No such file");
  });

  test("reads every template's files, so the agent loads the tested shape", async () => {
    const { tools } = setup();
    for (const id of templateChoices.map((choice) => choice.id)) {
      const files = applyTemplate(cloneStarter(), id).files;
      for (const name of ["index.html", "app.js", "schema.sql", "actions.json"] as const) {
        expect(textOf(await call(tools, "read", { path: `/templates/${id}/${name}` })), `${id}/${name}`).toBe(files[name]);
      }
    }
    const ranged = await call(tools, "read", { path: "/templates/webhook/actions.json", offset: 1, limit: 1 });
    expect(textOf(ranged)).toStartWith("/templates/webhook/actions.json lines 1-1 of");
  });

  test("names the templates when a template path is wrong, and never writes one", async () => {
    const { tools, workspace } = setup();
    await expect(call(tools, "read", { path: "/templates/kanban/app.js" })).rejects.toThrow("tasks, notes, members, form, webhook, digest, gallery");
    await expect(call(tools, "write", { path: "/templates/form/app.js", content: "x" })).rejects.toThrow();
    expect(workspace.changedFiles()).toEqual([]);
  });
});

describe("edit", () => {
  test("replaces a unique snippet and reports the first changed line", async () => {
    const { workspace, tools } = setup({ "styles.css": "a { color: red; }\nb { color: blue; }\n" });
    const result = await call(tools, "edit", { path: "/workspace/styles.css", old_string: "blue", new_string: "green" });
    expect(workspace.files["styles.css"]).toBe("a { color: red; }\nb { color: green; }\n");
    expect(result.details).toMatchObject({ path: "/workspace/styles.css", firstChangedLine: 2 });
    expect(workspace.changedFiles()).toEqual(["styles.css"]);
  });

  test("refuses an old_string that matches zero or several times", async () => {
    const { workspace, tools } = setup({ "styles.css": "a { color: red; }\nb { color: red; }\n" });
    await expect(call(tools, "edit", { path: "styles.css", old_string: "color: red;", new_string: "color: teal;" }))
      .rejects.toThrow("matches 2 times");
    await expect(call(tools, "edit", { path: "styles.css", old_string: "color: pink;", new_string: "color: teal;" }))
      .rejects.toThrow("was not found");
    await expect(call(tools, "edit", { path: "styles.css", old_string: "", new_string: "x" }))
      .rejects.toThrow("cannot be empty");
    await expect(call(tools, "edit", { path: "styles.css", old_string: "a {", new_string: "a {" }))
      .rejects.toThrow("identical");
    expect(workspace.changedFiles()).toEqual([]);
  });

  test("matches literally, including indentation and newlines", async () => {
    const { workspace, tools } = setup({ "app.js": "if (a) {\n  run();\n}\n" });
    await call(tools, "edit", { path: "app.js", old_string: "{\n  run();\n}", new_string: "{\n  walk();\n}" });
    expect(workspace.files["app.js"]).toBe("if (a) {\n  walk();\n}\n");
  });

  test("two edits to one file in a single batch both land", async () => {
    const { workspace, tools } = setup({ "styles.css": "a { color: red; }\nb { color: blue; }\n" });
    const [first, second] = await Promise.all([
      call(tools, "edit", { path: "styles.css", old_string: "red", new_string: "teal" }),
      call(tools, "edit", { path: "styles.css", old_string: "blue", new_string: "navy" }),
    ]);

    expect(workspace.files["styles.css"]).toBe("a { color: teal; }\nb { color: navy; }\n");
    expect(textOf(first)).toContain("Edited styles.css");
    expect(textOf(second)).toContain("Edited styles.css");
  });

  test("counts overlapping matches, so a self-overlapping old_string is ambiguous", async () => {
    const { workspace, tools } = setup({ "styles.css": "/* ---- */\n" });
    await expect(call(tools, "edit", { path: "styles.css", old_string: "--", new_string: "-" }))
      .rejects.toThrow("matches 3 times");
    expect(workspace.changedFiles()).toEqual([]);
  });

  test("edits a CRLF file from a snippet the model wrote with plain newlines", async () => {
    const { workspace, tools } = setup({ "app.js": "if (a) {\r\n  run();\r\n}\r\n" });
    await call(tools, "edit", { path: "app.js", old_string: "{\n  run();\n}", new_string: "{\n  walk();\n}" });
    expect(workspace.files["app.js"]).toBe("if (a) {\r\n  walk();\r\n}\r\n");
  });

  test("an aborted run cannot still write to the workspace", async () => {
    const { workspace, tools } = setup();
    const controller = new AbortController();
    controller.abort();

    await expect(call(tools, "edit", { path: "styles.css", old_string: "#e65337", new_string: "#3157d5" }, controller.signal))
      .rejects.toThrow("aborted");
    await expect(call(tools, "write", { path: "app.js", content: "// gone" }, controller.signal))
      .rejects.toThrow("aborted");
    expect(workspace.changedFiles()).toEqual([]);
  });

  test("reports a Capsule file that is missing from the record instead of crashing on undefined", async () => {
    const partial: Partial<CapsuleFiles> = { ...starterProject.files };
    delete partial["actions.json"];
    const tools = createCapsuleTools(createCapsuleWorkspace(partial as CapsuleFiles), createCapsuleValidator(loadSql));

    await expect(call(tools, "read", { path: "/workspace/actions.json" })).rejects.toThrow("No such file");
    await expect(call(tools, "edit", { path: "actions.json", old_string: "x", new_string: "y" }))
      .rejects.toThrow("No such file");
  });

  test("asks for validation after touching schema.sql", async () => {
    const { tools } = setup();
    const result = await call(tools, "edit", {
      path: "/workspace/schema.sql",
      old_string: "done INTEGER NOT NULL DEFAULT 0,",
      new_string: "done INTEGER NOT NULL DEFAULT 0,\n  due_at INTEGER,",
    });
    expect(textOf(result)).toContain("Call validate_capsule now.");
  });
});

describe("write", () => {
  test("replaces a whole Capsule file", async () => {
    const { workspace, tools } = setup();
    const result = await call(tools, "write", { path: "/workspace/index.html", content: "<main>new</main>" });
    expect(workspace.files["index.html"]).toBe("<main>new</main>");
    expect(textOf(result)).toContain("Wrote index.html");
  });

  test("creates nested files but keeps writes inside the workspace", async () => {
    const { workspace, tools } = setup();
    await call(tools, "write", { path: "/workspace/new.js", content: "export const value = 1;" });
    await call(tools, "write", { path: "/workspace/lib/app.js", content: "export const nested = true;" });
    await call(tools, "write", { path: "index.htm", content: "<main>extra</main>" });
    for (const path of ["/etc/passwd", "/workspace"]) {
      await expect(call(tools, "write", { path, content: "x" })).rejects.toThrow("safe Capsule file path");
    }
    for (const path of ["/workspace/app.js", "app.js", "./app.js"]) {
      await call(tools, "write", { path, content: "// ok" });
    }
    expect(workspace.changedFiles()).toEqual(["new.js", "lib/app.js", "index.htm", "app.js"]);
    expect(Object.keys(workspace.files).sort()).toEqual(["actions.json", "app.js", "index.htm", "index.html", "lib/app.js", "new.js", "schema.sql", "styles.css"]);
  });
});

describe("validate_capsule", () => {
  test("the real Gate renders generated component refusals with the shared words", async () => {
    const fixtures = wasmCases.cases.filter((item) => item.fixture);
    for (const fixture of fixtures) {
      const files = {
        ...starterProject.files,
        "functions.json": JSON.stringify({ version: 1, functions: { main: { entry: "functions/main.ts", mount: "/api", methods: ["GET"], access: "anonymous", actions: [] } } }),
        "functions/main.ts": "export const handle = () => new Response('ok');",
      };
      const workspace = createCapsuleWorkspace(files);
      const tools = createCapsuleTools(workspace, createCapsuleValidator(loadSql), {
        releaseFacts: async () => ({ modules: { functions: { on: true }, auth: { on: true } } }),
        functionPreparation: async () => {
          throw { refusal: { code: fixture.code, message: fixture.error, hint: fixture.hint } };
        },
      });
      const result = await call(tools, "validate_capsule", {});
      expect(result.details).toMatchObject({ ok: false, errors: [{ code: fixture.code, message: fixture.error, hint: fixture.hint }] });
      expect(textOf(result)).toContain(fixture.error);
      expect(textOf(result)).toContain(fixture.hint.remedy);
    }
  });

  test("renders the shared function refusal returned by owner MCP", async () => {
    const files = {
      ...starterProject.files,
      "functions.json": JSON.stringify({ version: 1, functions: { main: { entry: "functions/main.ts", mount: "/api", methods: ["GET"], access: "anonymous", actions: [] } } }),
      "functions/main.ts": "export const handle = () => new Response('ok');",
    };
    const workspace = createCapsuleWorkspace(files);
    const tools = createCapsuleTools(workspace, createCapsuleValidator(loadSql), {
      releaseFacts: async () => ({ modules: { functions: { on: true }, auth: { on: true } } }),
      functionPreparation: async () => { throw { refusal: { code: "function.wasm-init-pending", message: "A function must finish its top-level initialization before it can run.", hint: { fix: "capsule", remedy: "Make top-level initialization finish before the function handles requests." } } }; },
    });
    const result = await call(tools, "validate_capsule", {});
    expect(result.details).toMatchObject({ ok: false, errors: [{
      code: "function.wasm-init-pending",
      message: "A function must finish its top-level initialization before it can run.",
      hint: { fix: "capsule", remedy: "Make top-level initialization finish before the function handles requests." },
    }] });
    expect(textOf(result)).toContain("Make top-level initialization finish before the function handles requests.");
  });

  test("maps an editor-local function refusal by code", async () => {
    const files = {
      ...starterProject.files,
      "functions.json": JSON.stringify({ version: 1, functions: { main: { entry: "functions/main.ts", mount: "/api", methods: ["GET"], access: "anonymous", actions: [] } } }),
      "functions/main.ts": "export const handle = () => new Response('ok');",
    };
    const workspace = createCapsuleWorkspace(files);
    const localRefusal = refusal("function.wasm-init", "Function initialization failed.");
    const tools = createCapsuleTools(workspace, createCapsuleValidator(loadSql), {
      releaseFacts: async () => ({ modules: { functions: { on: true }, auth: { on: true } } }),
      functionPreparation: async () => { throw localRefusal; },
    });
    const result = await call(tools, "validate_capsule", {});
    expect(result.details).toMatchObject({ ok: false, errors: [{
      code: localRefusal.code,
      message: localRefusal.message,
      hint: localRefusal.hint,
    }] });
  });

  test("keeps a disconnected function preparation pending", async () => {
    const files = {
      ...starterProject.files,
      "functions.json": JSON.stringify({ version: 1, functions: { main: { entry: "functions/main.ts", mount: "/api", methods: ["GET"], access: "anonymous", actions: [] } } }),
      "functions/main.ts": "export const handle = () => new Response('ok');",
    };
    const workspace = createCapsuleWorkspace(files);
    const tools = createCapsuleTools(workspace, createCapsuleValidator(loadSql), {
      releaseFacts: async () => ({ modules: { functions: { on: true }, auth: { on: true } } }),
      functionPreparation: async () => { throw new Error("connection lost"); },
    });
    const result = await call(tools, "validate_capsule", {});
    expect(result.details).toMatchObject({ ok: true, functionPreparation: { status: "pending" } });
    expect(textOf(result)).toContain("preparation is pending");
  });

  test("reports a passing capsule", async () => {
    const { tools } = setup();
    const result = await call(tools, "validate_capsule", {});
    expect(result.details).toMatchObject({ ok: true, errors: [] });
    expect(textOf(result)).toContain("Validation passed");
  });

  test("reports the staged errors of a broken capsule as tool output, not a throw", async () => {
    const { workspace, tools } = setup();
    await call(tools, "write", { path: "/workspace/schema.sql", content: "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY);" });
    const result = await call(tools, "validate_capsule", {});
    expect((result.details as { ok: boolean }).ok).toBe(false);
    const text = textOf(result);
    expect(text).toContain("Validation failed");
    expect(text).toContain("[actions]");
    expect(text).toContain("tasks.list");
    expect(workspace.changedFiles()).toEqual(["schema.sql"]);
  });

  test("validates the live workspace, not the seed", async () => {
    const { workspace, tools } = setup({ "actions.json": "{ not json" });
    const first = await call(tools, "validate_capsule", {});
    expect((first.details as { ok: boolean }).ok).toBe(false);

    await call(tools, "write", { path: "/workspace/actions.json", content: starterProject.files["actions.json"] });
    const second = await call(tools, "validate_capsule", {});
    expect((second.details as { ok: boolean }).ok).toBe(true);
    expect(workspace.changedFiles()).toEqual(["actions.json"]);
  });

  test("creates, edits, reads back and validates a cron through the editor tools", async () => {
    const workspace = createCapsuleWorkspace({ "index.html": "<main>Cron</main>" });
    const tools = createCapsuleTools(workspace, createCapsuleValidator(loadSql), {
      releaseFacts: async () => ({ modules: { functions: { on: false }, actions: { on: false } } }),
    });
    const reference = await call(tools, "read", { path: "/reference/sdk/functions.md" });
    expect(textOf(reference)).toContain("Declared with \"crons\" in crons.json");
    await call(tools, "write", { path: "/workspace/functions/cleanup.js", content: "export function handle() { return new Response(null, { status: 204 }); }" });
    await call(tools, "write", { path: "/workspace/functions.json", content: JSON.stringify({
      version: 1, functions: { cleanup: { entry: "functions/cleanup.js", access: "scheduled", actions: ["sessions.expire"] } },
    }) });
    await call(tools, "write", { path: "/workspace/schema.sql", content: "CREATE TABLE IF NOT EXISTS sessions (id INTEGER PRIMARY KEY);" });
    await call(tools, "write", { path: "/workspace/actions.json", content: JSON.stringify({
      version: 1, actions: { "sessions.expire": { kind: "mutation", access: "scheduled", statement: "DELETE FROM sessions", bindings: [] } },
    }) });

    const path = "/workspace/crons.json";
    const draft = { version: 1, crons: { cleanup: { cron: "0 24 * * *", function: "cleanup" } } };
    const written = await call(tools, "write", { path, content: JSON.stringify(draft) });
    expect(textOf(written)).toContain("Call validate_capsule now.");
    const refused = await call(tools, "validate_capsule", {});
    expect(refused.details).toMatchObject({ ok: false, errors: [{ code: "cron.expression-invalid" }] });

    const cron = "0 8 * * 1-5";
    const edited = await call(tools, "edit_json", { path, changes: [
      { op: "replace", pointer: "/crons/cleanup/cron", expected: draft.crons.cleanup.cron, value: cron },
    ] });
    expect(textOf(edited)).toContain("Call validate_capsule now.");
    const read = await call(tools, "read", { path });
    expect(JSON.parse(textOf(read))).toEqual({ version: 1, crons: { cleanup: { cron, function: "cleanup" } } });
    const accepted = await call(tools, "validate_capsule", {});
    expect(accepted.details).toMatchObject({ ok: true });
  });
});

describe("formatValidation", () => {
  test("renders stage, message and detail for repair", () => {
    const text = formatValidation({
      ok: false,
      checked: [],
      errors: [{ stage: "schema", message: "schema.sql does not run on an empty database.", detail: 'near ",": syntax error' }],
    });
    expect(text).toBe(
      "Validation failed with 1 problem:\n- [schema] schema.sql does not run on an empty database.\n  near \",\": syntax error",
    );
  });

  test("a refused Capsule names its code and remedy, so the model can name the rule", () => {
    const text = formatValidation({
      ok: false,
      checked: ["schema"],
      errors: [{
        stage: "actions", message: "actions.json is not a valid server action program.",
        detail: "Action notes.list reads rows and has no LIMIT. Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.",
        code: "query.limit-missing", hint: { fix: "capsule", remedy: "Add a LIMIT to the outer SELECT. A total with no GROUP BY needs none." },
      }],
    });
    expect(text).toEndWith("\n  Code query.limit-missing: Add a LIMIT to the outer SELECT. A total with no GROUP BY needs none.");
  });

  test("a pass says which stages did not run, so the model cannot assume them", () => {
    expect(formatValidation({ ok: true, errors: [], checked: ["schema", "actions", "preview-rows"] }))
      .not.toContain("did not run");
    expect(formatValidation({ ok: true, errors: [], checked: ["schema", "actions"] }))
      .toContain("The preview-rows stage did not run");
  });
});

describe("release facts", () => {
  const route = JSON.stringify({ version: 1, actions: { "orders.receive": { kind: "mutation", access: "anonymous", statement: "INSERT INTO tasks (title) VALUES (?1)", bindings: [{ source: "input", key: "title", type: "string" }], http: { method: "POST", path: "/hooks/orders", signature: { header: "X-Signature", secret: "orders_hook" } } } } });

  test("validate_capsule passes the facts to the Gate, and an unset secret is a warning that names Access > Integrations", async () => {
    const workspace = createCapsuleWorkspace({ ...starterProject.files, "actions.json": route });
    const tools = createCapsuleTools(workspace, createCapsuleValidator(loadSql), { releaseFacts: async () => ({ secrets: [], modules: { files: { on: true } } }) });
    const result = await call(tools, "validate_capsule", {});
    expect((result.details as { ok: boolean }).ok).toBe(true);
    const text = textOf(result);
    expect(text).toContain("Validation passed");
    expect(text).toContain("Action orders.receive signs with the secret orders_hook, which is not set for this project. Set it with secret_set before publishing.");
    expect(text).toContain("Access > Integrations");
    expect(text).toContain("Tell the person");
  });

  test("a fact that cannot be read leaves the Gate silent on it, never blocked", async () => {
    expect(await readReleaseFacts({})).toEqual({});
    expect(await readReleaseFacts({ releaseFacts: async () => { throw new Error("signed out"); } })).toEqual({});
    expect(await readReleaseFacts({ releaseFacts: async () => ({ secrets: [{ name: "a" }] }) })).toEqual({ secrets: [{ name: "a" }] });
  });
});

describe("readLiveDatabase", () => {
  test("tells a missing database apart from an unreadable one", async () => {
    expect(await readLiveDatabase({})).toEqual({ state: "absent" });
    expect(await readLiveDatabase({ liveDatabase: async () => undefined })).toEqual({ state: "absent" });

    const bytes = new Uint8Array([1, 2, 3]);
    expect(await readLiveDatabase({ liveDatabase: async () => bytes })).toEqual({ state: "loaded", bytes });

    const failed = await readLiveDatabase({ liveDatabase: async () => { throw new Error("wasm heap is gone"); } });
    expect(failed).toEqual({ state: "unreadable", reason: "wasm heap is gone" });
  });
});

const _typecheck: (workspace: CapsuleWorkspace, validator: CapsuleValidator) => AgentTool<any>[] = createCapsuleTools;
void _typecheck;

describe("edit_json", () => {
  test("changes repeated bindings by pointer and refuses stale edits atomically", async () => {
    const {workspace,tools}=setup({"actions.json":JSON.stringify({actions:{"clients.create":{bindings:[{key:"email",type:"string"}]},"clients.update":{bindings:[{key:"email",type:"string"}]}}})});
    await call(tools,"edit_json",{path:"/workspace/actions.json",changes:[{op:"add",pointer:"/actions/clients.create/bindings/0/nullable",value:true},{op:"add",pointer:"/actions/clients.update/bindings/0/nullable",value:true}]});
    const saved=workspace.files["actions.json"];
    expect(JSON.parse(saved).actions["clients.create"].bindings[0].nullable).toBe(true);
    await expect(call(tools,"edit_json",{path:"actions.json",changes:[{op:"replace",pointer:"/actions/clients.create/bindings/0/type",expected:"string",value:"number"},{op:"replace",pointer:"/actions/clients.update/bindings/0/type",expected:"boolean",value:"number"}]})).rejects.toThrow("expected");
    expect(workspace.files["actions.json"]).toBe(saved);
  });
});


test("SDK references are readable without a platform session and remain outside the Capsule", async () => {
  const {tools}=setup();
  const reference=textOf(await call(tools,"read",{path:"/reference/sdk/files.md"}));
  expect(reference).toContain("miniapp.files.upload");
  await expect(call(tools,"write",{path:"/reference/sdk/files.md",content:"changed"})).rejects.toThrow();
});
