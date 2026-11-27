import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import path from "node:path";
import initSqlJs, { type SqlJsStatic } from "sql.js";
import { withPrivateNotes } from "../lib/private-notes";
import { cloneStarter, starterProject } from "../lib/starter";
import type { CapsuleFiles, LiveDatabase } from "./contracts";
import { createCapsuleValidator } from "./gate";
import { expectedOutcome } from "../kernel/refusal-cases";
import { formatValidation } from "./tools";

let sql: Promise<SqlJsStatic> | undefined;
const loadSql = () => (sql ??= initSqlJs());

const gate = createCapsuleValidator(loadSql);
const validator = {
  validate: (files: CapsuleFiles, live?: LiveDatabase, facts: Parameters<typeof gate.validate>[2] = {}) =>
    gate.validate(files, live, { modules: { files: { on: true } }, ...facts }),
};

function capsule(overrides: Partial<CapsuleFiles> = {}): CapsuleFiles {
  return { ...starterProject.files, ...overrides };
}

function actions(source: unknown): string {
  return JSON.stringify(source);
}

function loaded(bytes: Uint8Array): LiveDatabase {
  return { state: "loaded", bytes };
}

async function exportDatabase(schema: string): Promise<Uint8Array> {
  const SQL = await loadSql();
  const db = new SQL.Database();
  db.run(schema);
  const bytes = db.export();
  db.close();
  return bytes;
}

describe("validation gate", () => {
  test("uses files on a first release only when no state exists", async () => {
    const files = capsule({ "app.js": "await miniapp.files.upload(file);" });
    expect((await validator.validate(files)).ok).toBe(true);
    expect((await validator.validate(files, { state: "absent" }, { modules: {} })).ok).toBe(true);
    const result = await validator.validate(files, { state: "absent" }, { modules: { files: { on: false } } });
    expect(result.errors[0]).toMatchObject({ code: "module.off", message: "Module files is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run." });
    expect((await validator.validate(files, { state: "absent" }, { modules: { files: { on: true } } })).ok).toBe(true);
  });
  test("an action declaration needs the actions module", async () => {
    const files = capsule({ "actions.json": JSON.stringify({ version: 1, actions: { "notes.list": { kind: "query" } } }) });
    const result = await validator.validate(files, { state: "absent" }, { modules: { actions: { on: false } } });
    expect(result.errors[0]).toMatchObject({ code: "module.off", message: "Module actions is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run." });
  });
  test("HTTP routes and call actions refuse when their modules are off", async () => {
    const route = capsule({ "actions.json": JSON.stringify({ version: 1, actions: { "hooks.receive": { kind: "mutation", http: { method: "POST", path: "/hooks" } } } }) });
    const call = capsule({ "actions.json": JSON.stringify({ version: 1, actions: { "notify.send": { kind: "call" } } }) });
    for (const [files, module] of [[route, "http"], [call, "calls"]] as const) {
      const result = await validator.validate(files, { state: "absent" }, { modules: { actions: { on: true }, http: { on: false }, calls: { on: false } } });
      expect(result.errors[0]).toMatchObject({ code: "module.off", message: `Module ${module} is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run.` });
    }
  });
  test("a module that requires actions refuses an invalid first-release state", async () => {
    for (const module of ["http", "calls"] as const) {
      const result = await validator.validate(capsule({ "actions.json": JSON.stringify({ version: 1, actions: {} }), "schema.sql": "", "app.js": "" }), { state: "absent" }, { modules: { actions: { on: false }, [module]: { on: true } } });
      expect(result.errors[0]).toMatchObject({ code: "module.requires", message: `Module ${module} requires actions, which is off. Turn actions on first.` });
    }
  });
  test("checks files before actions when both modules are off", async () => {
    const files = capsule({
      "actions.json": JSON.stringify({ version: 1, actions: { "notes.list": { kind: "query" } } }),
      "app.js": "await miniapp.files.list();",
    });
    const result = await validator.validate(files, undefined, { modules: { actions: { on: false }, files: { on: false } } });
    expect(result.errors[0]).toMatchObject({ code: "module.off", message: "Module files is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run." });
  });
  test("module use refusals follow the shared release check order", async () => {
    const cases = JSON.parse(readFileSync(path.resolve(import.meta.dir, "../../kernel-rs/kernel/src/module-cases.json"), "utf8")) as Array<{ rule: string; order?: string[] }>;
    const order = cases.find((item) => item.rule === "release-order")?.order;
    expect(order).toEqual(["files", "actions", "http", "calls", "documents"]);
    const files = capsule({
      "app.js": "await miniapp.files.list();",
      "actions.json": JSON.stringify({ version: 1, actions: { "hooks.receive": { kind: "mutation", http: { method: "POST", path: "/hooks" } } } }),
    });
    const modules = Object.fromEntries(order!.map((module) => [module, { on: false }]));
    const result = await validator.validate(files, { state: "absent" }, { modules });
    expect(result.errors[0]).toMatchObject({ code: "module.off", message: "Module files is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run." });
  });
  test("a static capsule can omit actions.json and schema.sql when actions are off", async () => {
    const files = capsule();
    delete files["actions.json"];
    delete files["schema.sql"];
    const result = await validator.validate(files, { state: "absent" }, { modules: { seo: { on: true } }, visibility: "public" });
    expect(result.ok).toBe(true);
  });
  test("requires auth when actions name an authenticated caller or actor binding", async () => {
    const files = capsule({ "app.js": "await miniapp.auth.session();" });
    expect((await validator.validate(files)).ok).toBe(true);
    const result = await validator.validate(files, undefined, { modules: { files: { on: false }, auth: { on: false } } });
    expect(result.errors[0]).toMatchObject({ code: "module.off", message: "Module auth is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run." });
  });

  test("file storage requires auth when the app turns it on", async () => {
    const result = await validator.validate(capsule({ "app.js": "await miniapp.files.upload(file);" }), undefined, { modules: { files: { on: true }, auth: { on: false } } });
    expect(result.errors[0]).toMatchObject({ code: "module.requires", message: "Module files requires auth, which is off. Turn auth on first." });
  });
  test("a private first release refuses an explicitly off auth module", async () => {
    const result = await validator.validate(capsule(), { state: "absent" }, { visibility: "private", modules: { auth: { on: false } } });
    expect(result.errors[0]).toMatchObject({ code: "module.requires", message: "A private app requires auth, which is off. Turn auth on first." });
  });
  test("passes the starter capsule", async () => {
    const result = await validator.validate(capsule());
    expect(result.errors).toEqual([]);
    expect(result.ok).toBe(true);
  });

  test("refuses a proposal whose changed text and retained binary file exceed the source count", async () => {
    const files = { ...starterProject.files };
    for (let index = 0; Object.keys(files).length < 1_000; index += 1) files[`extra-${index}.txt`] = "";
    const project = {
      files: { ...starterProject.files, "public/logo.bin": Buffer.from([0, 255, 1]).toString("base64") },
      encodings: { "public/logo.bin": "base64" as const },
    };
    const result = await validator.validate(files, { state: "absent" }, { sourceFiles: project });

    expect(result.ok).toBe(false);
    expect(result.checked).toEqual([]);
    expect(result.errors[0]).toMatchObject({
      stage: "sources",
      code: "sources.file-count",
      message: "app/ holds 1001 files. A project's sources hold at most 1000 files.",
    });
    expect(result.errors[0].hint?.remedy).toBeTruthy();
  });

  test("counts retained binary bytes against the shared decoded source size", async () => {
    const bytes = Buffer.alloc(20 * 1024 * 1024 + 1, 255);
    const project = {
      files: { ...starterProject.files, "public/large.bin": bytes.toString("base64") },
      encodings: { "public/large.bin": "base64" as const },
    };
    const result = await validator.validate(capsule(), { state: "absent" }, { sourceFiles: project });

    expect(result.ok).toBe(false);
    expect(result.checked).toEqual([]);
    expect(result.errors[0].stage).toBe("sources");
    expect(result.errors[0].code).toBe("sources.size");
    expect(result.errors[0].message).toContain("A project's sources hold at most 20 MiB.");
    expect(result.errors[0].hint?.remedy).toBeTruthy();
  });

  test("warns, without failing, when a query returns an expression without a name", async () => {
    const result = await validator.validate(capsule({
      "schema.sql": "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL);",
      "actions.json": actions({ version: 1, actions: {
        "tasks.count": { kind: "query", access: "anonymous", statement: "SELECT count(*) FROM tasks", bindings: [] },
        "tasks.total": { kind: "query", access: "anonymous", statement: "SELECT count(*) AS n FROM tasks", bindings: [] },
      } }),
    }));
    expect(result.ok).toBe(true);
    expect(result.warnings).toEqual([{
      stage: "actions",
      message: "Server action tasks.count returns count(*) without a name. Add AS <name> so every backend returns the same key.",
    }]);
    expect(formatValidation(result)).toContain("Warning: Server action tasks.count returns count(*) without a name.");
    expect((await validator.validate(capsule())).warnings).toBeUndefined();
  });

  test("fails at stage schema when schema.sql cannot run", async () => {
    const result = await validator.validate(capsule({ "schema.sql": "CREATE TABLE tasks (id INTEGER,;" }));
    expect(result.ok).toBe(false);
    expect(result.errors).toHaveLength(1);
    expect(result.errors[0].stage).toBe("schema");
    expect(result.errors[0].message).toContain("schema.sql");
    expect(result.errors[0].detail).toBeTruthy();
  });

  test("fails at stage actions when an action reads a column the schema does not declare", async () => {
    const result = await validator.validate(capsule({
      "actions.json": actions({
        version: 1,
        actions: {
          "tasks.list": {
            kind: "query",
            access: "anonymous",
            statement: "SELECT id, title, due_at FROM tasks LIMIT 50",
            bindings: [],
          },
        },
      }),
    }));
    expect(result.ok).toBe(false);
    expect(result.errors.map((error) => error.stage)).toEqual(["actions"]);
    expect(result.errors[0].message).toContain("tasks.list");
    expect(result.errors[0].detail).toContain("due_at");
  });

  test("fails at stage actions when actions.json is not a valid program", async () => {
    const broken = await validator.validate(capsule({ "actions.json": "{ not json" }));
    expect(broken.ok).toBe(false);
    expect(broken.errors[0].stage).toBe("actions");
    expect(broken.errors[0].detail).toContain("valid JSON");

    const structural = await validator.validate(capsule({
      "actions.json": actions({
        version: 1,
        actions: { "tasks.wipe": { kind: "mutation", access: "anonymous", statement: "DROP TABLE tasks", bindings: [] } },
      }),
    }));
    expect(structural.ok).toBe(false);
    expect(structural.errors[0].stage).toBe("actions");
  });

  test("refuses an http binding the Kernel refuses and accepts one it serves", async () => {
    const program = (http: unknown, access = "anonymous") => actions({
      version: 1,
      actions: { "tasks.feed": { kind: "query", access, statement: "SELECT id, title FROM tasks LIMIT 50", bindings: [], http } },
    });
    const served = await validator.validate(capsule({ "actions.json": program({ method: "GET", path: "/api/tasks" }) }));
    expect(served.errors).toEqual([]);
    for (const [http, access, message] of [
      [{ method: "GET", path: "/api/tasks" }, "authenticated", "Action tasks.feed is authenticated, so it cannot declare http. Only anonymous actions answer on an HTTP route."],
      [{ method: "POST", path: "/api/tasks" }, "anonymous", "Action tasks.feed must bind GET for a query and POST for a mutation."],
      [{ method: "GET", path: "/_konae/files" }, "anonymous", "Action tasks.feed binds the reserved path /_konae/files. Paths under /_konae, /oauth, /.well-known and /mcp belong to the Kernel."],
    ] as const) {
      const refused = await validator.validate(capsule({ "actions.json": program(http, access) }));
      expect(refused.ok).toBe(false);
      expect(refused.errors[0].stage).toBe("actions");
      expect(refused.errors[0].detail).toBe(message);
    }
  });

  test("passes a signal action without SQL and refuses a signal the Kernel refuses", async () => {
    const program = (signal: unknown, kind = "signal") => actions({
      version: 1,
      actions: {
        ...JSON.parse(starterProject.files["actions.json"]).actions,
        "tasks.like": { kind, access: "anonymous", bindings: [{ source: "input", key: "title", type: "string", maxLength: 80 }],
          ...(kind === "query" ? { statement: "SELECT id FROM tasks LIMIT 5" } : {}), signal },
      },
    });
    const passed = await validator.validate(capsule({ "actions.json": program({ name: "task.liked", properties: ["title"] }) }));
    expect(passed.errors).toEqual([]);
    for (const [signal, kind, message] of [
      [{ name: "task.liked", properties: ["due"] }, "signal", "Action tasks.like names signal property due, which is not one of its input bindings."],
      [{ name: "task.liked" }, "query", 'Action tasks.like is a query, so it cannot declare a signal. Declare it on a mutation or on a "kind": "signal" action.'],
      [{ name: "Liked" }, "signal", 'Action tasks.like names an invalid signal. Use dot-separated lowercase words, such as "order.placed", up to 64 characters.'],
    ] as const) {
      const refused = await validator.validate(capsule({ "actions.json": program(signal, kind) }));
      expect(refused.ok).toBe(false);
      expect(refused.errors[0].stage).toBe("actions");
      expect(refused.errors[0].detail).toBe(message);
    }
  });

  test("fails when declared bindings and statement placeholders disagree", async () => {
    const tooFew = await validator.validate(capsule({
      "actions.json": actions({
        version: 1,
        actions: {
          "tasks.create": {
            kind: "mutation",
            access: "anonymous",
            statement: "INSERT INTO tasks (title, done, created_at) VALUES (?, 0, ?)",
            bindings: [{ source: "input", key: "title", type: "string" }],
          },
        },
      }),
    }));
    expect(tooFew.ok).toBe(false);
    expect(tooFew.errors[0].stage).toBe("actions");
    expect(tooFew.errors[0].message).toContain("more parameters");

    const tooMany = await validator.validate(capsule({
      "actions.json": actions({
        version: 1,
        actions: {
          "tasks.toggle": {
            kind: "mutation",
            access: "anonymous",
            statement: "UPDATE tasks SET done = 1 WHERE id = ?",
            bindings: [
              { source: "input", key: "id", type: "number" },
              { source: "kernel", field: "now" },
            ],
          },
        },
      }),
    }));
    expect(tooMany.ok).toBe(false);
    expect(tooMany.errors[0].message).toContain("fewer parameters");
  });

  test("accepts nullable filters and refuses mutation RETURNING", async () => {
    const filtered = await validator.validate(capsule({ "actions.json": actions({ version: 1, actions: {
      "tasks.filter": {
        kind: "query", access: "anonymous",
        statement: "SELECT id, title FROM tasks WHERE (?1 IS NULL OR title LIKE '%' || ?1 || '%') LIMIT 50",
        bindings: [{ source: "input", key: "q", type: "string", nullable: true }],
      },
    } }) }));
    expect(filtered.ok).toBe(true);
    const returning = await validator.validate(capsule({ "actions.json": actions({ version: 1, actions: {
      "tasks.remove": { kind: "mutation", access: "anonymous", statement: "DELETE FROM tasks RETURNING id", bindings: [] },
    } }) }));
    expect(returning.ok).toBe(false);
    expect(returning.errors[0].stage).toBe("actions");
    expect(returning.errors[0].detail).toContain("cannot use RETURNING");
  });

  test("accepts a valid schema and action change together", async () => {
    const result = await validator.validate(capsule({
      "schema.sql": "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, owner_id TEXT NOT NULL, body TEXT NOT NULL);",
      "actions.json": actions({
        version: 1,
        actions: {
          "notes.list": {
            kind: "query",
            access: "authenticated",
            statement: "SELECT id, body FROM notes WHERE owner_id = ? ORDER BY id DESC LIMIT 100",
            bindings: [{ source: "actor", field: "id" }],
          },
          "notes.create": {
            kind: "mutation",
            access: "authenticated",
            statement: "INSERT INTO notes (owner_id, body) VALUES (?, ?)",
            bindings: [
              { source: "actor", field: "id" },
              { source: "input", key: "body", type: "string", maxLength: 500 },
            ],
          },
        },
      }),
    }));
    expect(result.errors).toEqual([]);
    expect(result.ok).toBe(true);
  });

  test("stage preview-rows accepts a live database that already matches the schema", async () => {
    const live = await exportDatabase(starterProject.files["schema.sql"]);
    const result = await validator.validate(capsule(), loaded(live));
    expect(result.errors).toEqual([]);
    expect(result.ok).toBe(true);
  });

  test("stage preview-rows warns that the preview is behind a column the Kernel adds at publish, and passes", async () => {
    const live = await exportDatabase("CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL);");
    const result = await validator.validate(capsule({
      "schema.sql": "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, due_at INTEGER);",
      "actions.json": actions({
        version: 1,
        actions: {
          "tasks.list": {
            kind: "query",
            access: "anonymous",
            statement: "SELECT id, title, due_at FROM tasks LIMIT 100",
            bindings: [],
          },
        },
      }),
    }), loaded(live));

    expect(result.errors).toEqual([]);
    expect(result.ok).toBe(true);
    expect(result.checked).not.toContain("preview-rows");
    expect(result.checked).toEqual(["sources", "schema", "actions"]);
    expect(result.warnings).toEqual([expect.objectContaining({ stage: "preview-rows", message: expect.stringContaining("Publishing applies it"), detail: "Its table tasks has no due_at." })]);
  });

  test("stage schema rejects a schema that cannot be re-run, with or without a live database", async () => {
    const files = capsule({
      "schema.sql": starterProject.files["schema.sql"].replace("CREATE TABLE IF NOT EXISTS", "CREATE TABLE"),
    });

    const fresh = await validator.validate(files);
    expect(fresh.ok).toBe(false);
    expect(fresh.errors).toHaveLength(1);
    expect(fresh.errors[0].stage).toBe("schema");
    expect(fresh.errors[0].message).toContain("not idempotent");
    expect(fresh.errors[0].detail).toContain("already exists");

    const live = await validator.validate(files, loaded(await exportDatabase(starterProject.files["schema.sql"])));
    expect(live.errors[0].stage).toBe("schema");
  });

  test("stage schema refuses a schema.sql that destroys what is already there", async () => {
    const dropped = await validator.validate(
      capsule({ "schema.sql": "DROP TABLE IF EXISTS tasks;\nCREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL);" }),
      loaded(await exportDatabase(starterProject.files["schema.sql"])),
    );
    expect(dropped.ok).toBe(false);
    expect(dropped.errors[0].stage).toBe("schema");
    expect(dropped.errors[0].message).toContain("DROP");

    const deleted = await validator.validate(capsule({
      "schema.sql": `${starterProject.files["schema.sql"]}\nDELETE FROM tasks WHERE done = 1;`,
    }));
    expect(deleted.ok).toBe(false);
    expect(deleted.errors[0].message).toContain("DELETE");
  });

  test("refuses triggers and views, which the Kernel will not deploy", async () => {
    const trigger = await validator.validate(capsule({
      "schema.sql": `${starterProject.files["schema.sql"]}
CREATE TRIGGER IF NOT EXISTS tasks_wipe AFTER INSERT ON tasks
BEGIN
  DELETE FROM tasks WHERE id <> NEW.id;
END;`,
    }));
    expect(trigger.ok).toBe(false);
    expect(trigger.errors[0].stage).toBe("schema");
    expect(trigger.errors[0].message).toContain("CREATE TRIGGER");
    expect(trigger.errors[0].message).not.toContain("DELETE");

    const view = await validator.validate(capsule({
      "schema.sql": `${starterProject.files["schema.sql"]}\nCREATE VIEW IF NOT EXISTS open_tasks AS SELECT * FROM tasks WHERE done = 0;`,
    }));
    expect(view.errors[0].message).toContain("CREATE VIEW");

    const index = await validator.validate(capsule({
      "schema.sql": `${starterProject.files["schema.sql"]}\nCREATE UNIQUE INDEX IF NOT EXISTS tasks_title ON tasks (title);`,
    }));
    expect(index.ok).toBe(true);
  });

  test("refuses every statement the Kernel's schema authorizer refuses", async () => {
    const live = loaded(await exportDatabase(starterProject.files["schema.sql"]));
    for (const [statement, named] of [
      ["INSERT INTO tasks (id, title, done, created_at) VALUES (1, 'seed', 0, 0) ON CONFLICT DO NOTHING;", "INSERT"],
      ["INSERT OR IGNORE INTO tasks (id, title, done, created_at) VALUES (1, 'seed', 0, 0);", "INSERT"],
      ["INSERT INTO tasks (title, done, created_at) SELECT 'x', 0, 0 WHERE NOT EXISTS (SELECT 1 FROM tasks);", "INSERT"],
      ["INSERT INTO tasks (title, done, created_at) VALUES ('seed', 0, 0);", "INSERT"],
      ["REPLACE INTO tasks (id, title, done, created_at) VALUES (1, 'seed', 0, 0);", "REPLACE"],
      ["PRAGMA foreign_keys = ON;", "PRAGMA"],
      ["BEGIN;\nCOMMIT;", "BEGIN"],
      ["CREATE TEMP TABLE scratch (id INTEGER);", "CREATE TEMP"],
      ["CREATE TABLE IF NOT EXISTS copy AS SELECT * FROM tasks;", "CREATE TABLE ... AS"],
    ]) {
      const result = await validator.validate(capsule({ "schema.sql": `${starterProject.files["schema.sql"]}\n${statement}` }), live);
      expect(result.ok).toBe(false);
      expect(result.errors[0].stage).toBe("schema");
      expect(result.errors[0].message).toContain(named);
      expect(result.errors[0].message).not.toContain("seed");
    }
  });

  test("refuses every CREATE TABLE ... AS form and the temp schema", async () => {
    for (const [statement, named] of [
      ["CREATE TABLE IF NOT EXISTS copy AS VALUES (1);", "CREATE TABLE ... AS"],
      ["CREATE TABLE IF NOT EXISTS copy AS WITH x AS (SELECT 1) SELECT * FROM x;", "CREATE TABLE ... AS"],
      ["CREATE TABLE IF NOT EXISTS temp.scratch (a);", "CREATE TABLE temp.*"],
      ['CREATE TABLE IF NOT EXISTS "TEMPORARY".scratch (a);', "CREATE TABLE TEMPORARY.*"],
      ["CREATE TABLE temp.scratch (a);", "CREATE TABLE temp.*"],
    ]) {
      const result = await validator.validate(capsule({ "schema.sql": `${starterProject.files["schema.sql"]}\n${statement}` }));
      expect(result.ok).toBe(false);
      expect(result.errors[0].stage).toBe("schema");
      expect(result.errors[0].message).toContain(named);
    }
  });

  test("allows SQL functions in schema and refuses the dangerous ones", async () => {
    for (const statement of [
      "CREATE TABLE IF NOT EXISTS labels (id INTEGER PRIMARY KEY, name TEXT CHECK (length(name) > 0));",
      "CREATE TABLE IF NOT EXISTS shouts (a TEXT, b TEXT GENERATED ALWAYS AS (upper(a)));",
      "CREATE TABLE IF NOT EXISTS stamps (id INTEGER PRIMARY KEY, at TEXT DEFAULT (datetime('now')));",
      "CREATE INDEX IF NOT EXISTS tasks_lower_title ON tasks(lower(title));",
      "CREATE INDEX IF NOT EXISTS tasks_open ON tasks(done) WHERE length(title) > 0;",
      "CREATE TABLE IF NOT EXISTS notes (as_of TEXT, readfile TEXT);",
      "CREATE TABLE IF NOT EXISTS main.mains (a);",
    ]) {
      const result = await validator.validate(capsule({ "schema.sql": `${starterProject.files["schema.sql"]}\n${statement}` }));
      expect({ statement, errors: result.errors }).toEqual({ statement, errors: [] });
    }
    for (const [statement, named] of [
      ["CREATE TABLE IF NOT EXISTS files (path TEXT CHECK (readfile(path) IS NULL));", "readfile()"],
      ["CREATE TABLE IF NOT EXISTS files (path TEXT CHECK (WRITEFILE(path, 'x') > 0));", "writefile()"],
      ['CREATE TABLE IF NOT EXISTS files (path TEXT CHECK ("load_extension"(path) IS NULL));', "load_extension()"],
      ["CREATE INDEX IF NOT EXISTS tasks_tok ON tasks(fts3_tokenizer(title));", "fts3_tokenizer()"],
    ]) {
      const result = await validator.validate(capsule({ "schema.sql": `${starterProject.files["schema.sql"]}\n${statement}` }));
      expect(result.ok).toBe(false);
      expect(result.errors[0].stage).toBe("schema");
      expect(result.errors[0].message).toContain(named);
    }
  });

  test("the starter and private notes schemas pass", async () => {
    expect((await validator.validate(capsule())).ok).toBe(true);
    expect((await validator.validate(withPrivateNotes(cloneStarter()).files as CapsuleFiles)).ok).toBe(true);
  });

  test("a verdict names the stages that actually ran", async () => {
    const withoutDatabase = await validator.validate(capsule());
    expect(withoutDatabase.ok).toBe(true);
    expect(withoutDatabase.checked).toEqual(["sources", "schema", "actions"]);

    const withDatabase = await validator.validate(capsule(), loaded(await exportDatabase(starterProject.files["schema.sql"])));
    expect(withDatabase.ok).toBe(true);
    expect(withDatabase.checked).toEqual(["sources", "schema", "actions", "preview-rows"]);

    const unreadable = await validator.validate(capsule(), { state: "unreadable", reason: "the wasm heap is gone" });
    expect(unreadable.ok).toBe(false);
    expect(unreadable.checked).not.toContain("preview-rows");
    expect(unreadable.errors[0].stage).toBe("preview-rows");
    expect(unreadable.errors[0].detail).toContain("wasm heap");
  });

  test("stage preview-rows warns when schema.sql does not run on the preview's older table", async () => {
    const live = await exportDatabase("CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL);");
    const result = await validator.validate(capsule({
      "schema.sql": "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, due_at INTEGER);\nCREATE INDEX IF NOT EXISTS idx_tasks_due ON tasks (due_at);",
    }), loaded(live));

    expect(result.errors.map((error) => error.stage)).not.toContain("preview-rows");
    expect(result.warnings?.find((warning) => warning.stage === "preview-rows")?.detail).toContain("schema.sql does not run on it");
  });
});

describe("the gate and the Kernel refuse the same actions.json", () => {
  const caseSchema = [
    "CREATE TABLE IF NOT EXISTS orders (id INTEGER PRIMARY KEY, external_id TEXT, total REAL, paid INTEGER, user_id TEXT, created_at INTEGER, note TEXT);",
    "CREATE TABLE IF NOT EXISTS members (id TEXT PRIMARY KEY, role TEXT);",
    "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, body TEXT, user_id TEXT);",
    "CREATE TABLE IF NOT EXISTS bookings (slot INTEGER, user_id TEXT, room_id INTEGER, cancelled_at INTEGER);",
    "CREATE TABLE IF NOT EXISTS log (user_id TEXT);",
    "CREATE TABLE IF NOT EXISTS tags (who TEXT);",
    "CREATE TABLE IF NOT EXISTS joins (source TEXT);",
    "CREATE TABLE IF NOT EXISTS checkins (habit_id INTEGER, user_id TEXT, day INTEGER);",
    "CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, acknowledged_at INTEGER);",
    "CREATE TABLE IF NOT EXISTS holds (id INTEGER PRIMARY KEY, room TEXT, kind TEXT, until INTEGER, released_at INTEGER);",
    "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT, assignee_id TEXT, due_day TEXT, reminded_at INTEGER, a, b, c, d, e, f, g, h);",
  ].join("\n");

  test("every case in kernel-rs/kernel/src/notify-column-cases.json", async () => {
    const cases: { case: string; schema: string; actions: Record<string, unknown>; error: string | null; code?: string }[] =
      await Bun.file(new URL("../../kernel-rs/kernel/src/notify-column-cases.json", import.meta.url)).json();
    expect(cases.length).toBeGreaterThanOrEqual(8);
    for (const item of cases) {
      const result = await validator.validate(capsule({ "schema.sql": item.schema, "actions.json": actions({ version: 1, actions: item.actions }) }));
      expect(result.errors.map((error) => ({ code: error.code, message: error.message })), item.case)
        .toEqual(item.error === null ? [] : [expectedOutcome(item)]);
    }
  });

  const files = ["action-cases.json", "sql-rule-cases.json", "http-binding-cases.json", "signal-cases.json", "guard-cases.json", "notify-cases.json", "schedule-cases.json", "documents-cases.json"];

  for (const file of files) {
    test(`every case in kernel-rs/kernel/src/${file}`, async () => {
      const cases: { case: string; actions?: Record<string, unknown>; schema?: string; error: string | null; code?: string }[] =
        await Bun.file(new URL(`../../kernel-rs/kernel/src/${file}`, import.meta.url)).json();
      expect(cases.length).toBeGreaterThan(5);
      for (const item of cases) {
        if (item.schema !== undefined && item.actions === undefined) {
          const result = await validator.validate(capsule({ "schema.sql": item.schema, "actions.json": actions({ version: 1, actions: {} }) }));
          expect(result.errors.map((error) => ({ code: error.code, message: error.message })), item.case).toEqual(item.error === null ? [] : [expectedOutcome(item)]);
          continue;
        }
        const result = await validator.validate(capsule({ "schema.sql": item.schema ?? caseSchema, "actions.json": actions({ version: 1, actions: item.actions }) }));
        if (item.error === null) {
          expect(result.errors, item.case).toEqual([]);
        } else {
          expect(result.errors.map((error) => ({ code: error.code, message: error.detail })), item.case).toEqual([expectedOutcome(item)]);
          expect(formatValidation(result), item.case).toContain(item.error);
        }
      }
    });
  }
});
