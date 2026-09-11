import { describe, expect, test } from "bun:test";
import initSqlJs, { type SqlJsStatic } from "sql.js";
import { starterProject } from "../lib/starter";
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

  test("a trigger body is not mistaken for a destructive statement", async () => {
    const result = await validator.validate(capsule({
      "schema.sql": `${starterProject.files["schema.sql"]}
CREATE TRIGGER IF NOT EXISTS tasks_touch AFTER INSERT ON tasks
BEGIN
  UPDATE tasks SET created_at = created_at WHERE id = NEW.id;
END;`,
    }));
    expect(result.errors).toEqual([]);
    expect(result.ok).toBe(true);
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
