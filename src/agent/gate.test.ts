import { describe, expect, test } from "bun:test";
import initSqlJs, { type SqlJsStatic } from "sql.js";
import { withPrivateNotes } from "../lib/private-notes";
import { cloneStarter, starterProject } from "../lib/starter";
import type { CapsuleFiles, LiveDatabase } from "./contracts";
import { createCapsuleValidator } from "./gate";

// sql.js resolves its own wasm under bun; the browser path uses the bundled url loader.
let sql: Promise<SqlJsStatic> | undefined;
const loadSql = () => (sql ??= initSqlJs());

const validator = createCapsuleValidator(loadSql);

function capsule(overrides: Partial<CapsuleFiles> = {}): CapsuleFiles {
  return { ...starterProject.files, ...overrides };
}

function actions(source: unknown): string {
  return JSON.stringify(source);
}

/** The gate's view of a live database it could read. */
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
  test("passes the starter capsule", async () => {
    const result = await validator.validate(capsule());
    expect(result.errors).toEqual([]);
    expect(result.ok).toBe(true);
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

  test("stage migration accepts a live database that already matches the schema", async () => {
    const live = await exportDatabase(starterProject.files["schema.sql"]);
    const result = await validator.validate(capsule(), loaded(live));
    expect(result.errors).toEqual([]);
    expect(result.ok).toBe(true);
  });

  test("stage migration reports a live table that cannot converge on its own", async () => {
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

    expect(result.ok).toBe(false);
    const stages = result.errors.map((error) => error.stage);
    expect(stages).toContain("migration");
    expect(stages).not.toContain("actions");
    expect(result.errors.find((error) => error.message.includes("due_at"))?.detail).toContain("migration");
    expect(result.errors.some((error) => error.message.includes("does not prepare against the current database"))).toBe(true);
  });

  test("stage schema rejects a schema that cannot be re-run, with or without a live database", async () => {
    // src/runtime/host.ts runs schema.sql on every database open, so one clean run proves nothing.
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

  /**
   * The bug this pins: `DROP TABLE IF EXISTS tasks;` above the new CREATE passed
   * the gate. The migration stage runs on a copy, so the destruction is invisible
   * there, and Apply then runs the same script against the owner's real database.
   */
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

  /**
   * The bug this pins: only the leading keyword was checked, so a trigger that
   * wiped rows on every seed INSERT passed, and the Kernel's schema authorizer,
   * which only allows CREATE TABLE and CREATE INDEX, then refused it at deploy.
   */
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

  /**
   * The bug this pins: the gate allowed INSERT ... ON CONFLICT DO NOTHING,
   * PRAGMA and BEGIN/COMMIT in schema.sql, and the Kernel's schema authorizer,
   * which only allows CREATE TABLE and CREATE INDEX, then refused the deploy.
   */
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

  /**
   * The bug this pins: the gate only looked for AS SELECT, so AS VALUES, AS
   * WITH and a table named in the temp schema passed and the Kernel refused
   * the deploy.
   */
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

  /** Kept in step with the Kernel's authorizer, which allows SQL functions in schema except the ones that reach the host. */
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

  /**
   * The bug this pins: an unreadable live database was swallowed into "there is
   * none", so the same capsule flipped from blocked to a full pass and the panel
   * still claimed the migration stage held.
   */
  test("a verdict names the stages that actually ran", async () => {
    const withoutDatabase = await validator.validate(capsule());
    expect(withoutDatabase.ok).toBe(true);
    expect(withoutDatabase.checked).toEqual(["schema", "actions"]);

    const withDatabase = await validator.validate(capsule(), loaded(await exportDatabase(starterProject.files["schema.sql"])));
    expect(withDatabase.ok).toBe(true);
    expect(withDatabase.checked).toEqual(["schema", "actions", "migration"]);

    const unreadable = await validator.validate(capsule(), { state: "unreadable", reason: "the wasm heap is gone" });
    expect(unreadable.ok).toBe(false);
    expect(unreadable.checked).not.toContain("migration");
    expect(unreadable.errors[0].stage).toBe("migration");
    expect(unreadable.errors[0].detail).toContain("wasm heap");
  });

  test("stage migration reports a live table an idempotent schema cannot converge with", async () => {
    // Runs twice on an empty database, but the live tasks table predates the index's column.
    const live = await exportDatabase("CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL);");
    const result = await validator.validate(capsule({
      "schema.sql": "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, due_at INTEGER);\nCREATE INDEX IF NOT EXISTS idx_tasks_due ON tasks (due_at);",
    }), loaded(live));

    expect(result.ok).toBe(false);
    expect(result.errors.map((error) => error.stage)).toContain("migration");
    expect(result.errors.some((error) => error.message.includes("does not run on the current database"))).toBe(true);
  });
});
