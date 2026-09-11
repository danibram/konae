import { describe, expect, test } from "bun:test";
import { classifySql, modelTableNames } from "./sql-statements";

describe("SQL console statements", () => {
  test("reads run without asking", () => {
    expect(classifySql("SELECT * FROM tasks WHERE 0")).toEqual({ read: true, danger: null });
    expect(classifySql("-- just looking\nPRAGMA table_info(tasks);")).toEqual({ read: true, danger: null });
    expect(classifySql("select 'drop table tasks' as text")).toEqual({ read: true, danger: null });
  });

  test("ordinary writes are writes but need no confirmation", () => {
    expect(classifySql("INSERT INTO tasks(title) VALUES ('a')")).toEqual({ read: false, danger: null });
    expect(classifySql("DELETE FROM tasks WHERE id = 1")).toEqual({ read: false, danger: null });
    expect(classifySql("PRAGMA user_version = 3").read).toBe(false);
  });

  /** The bug this pins: only `=` counted as setting a pragma, so the call form read as a read. */
  test("a pragma set through its argument form is a write", () => {
    expect(classifySql("PRAGMA writable_schema(1)").read).toBe(false);
    expect(classifySql("PRAGMA user_version(5)").read).toBe(false);
    expect(classifySql("PRAGMA main.user_version(5)").read).toBe(false);
    expect(classifySql("PRAGMA main.table_info(tasks)").read).toBe(true);
    expect(classifySql("PRAGMA index_list(tasks)").read).toBe(true);
    expect(classifySql("PRAGMA foreign_keys").read).toBe(true);
  });

  /** The bug this pins: `é` ended the word, so `éwhere` read as a WHERE clause on a delete that has none. */
  test("characters at or above U+0080 belong to the identifier around them", () => {
    expect(classifySql("DELETE FROM éwhere").danger).toContain("every row");
    expect(classifySql("DELETE FROM t\u00a0WHERE").danger).toContain("every row");
    expect(classifySql("UPDATE t SET a = 1 -- é\nWHERE id = 1").danger).toBeNull();
    expect(classifySql("SELECT * FROM 表 WHERE 1").read).toBe(true);
  });

  test("destructive statements ask first", () => {
    expect(classifySql("DROP TABLE tasks").danger).toContain("deletes tasks");
    expect(classifySql("delete from tasks").danger).toContain("every row");
    expect(classifySql("UPDATE tasks SET done = 1").danger).toContain("every row");
    expect(classifySql("ALTER TABLE tasks ADD note text").danger).toContain("ALTER TABLE");
    expect(classifySql("SELECT 1; DROP TABLE IF EXISTS notes;").read).toBe(false);
  });

  /** The bug this pins: comments were stripped before strings, so `'--'` hid the rest of the script. */
  test("a comment marker inside a string does not hide the next statement", () => {
    const kind = classifySql("SELECT '--'; DROP TABLE tasks");
    expect(kind.read).toBe(false);
    expect(kind.danger).toContain("deletes tasks");
    expect(classifySql("SELECT '/*'; DELETE FROM tasks; SELECT '*/'").danger).toContain("every row");
    expect(classifySql(`SELECT "--"; DROP TABLE notes`).danger).toContain("deletes notes");
    expect(classifySql("SELECT [a;b] FROM t; UPDATE tasks SET done = 1").danger).toContain("every row");
    expect(classifySql("SELECT 'a;b' -- ; DROP TABLE tasks\nFROM t").read).toBe(true);
  });

  test("every statement of a script is classified", () => {
    expect(classifySql("SELECT 1; SELECT 2;")).toEqual({ read: true, danger: null });
    expect(classifySql("SELECT 1; INSERT INTO tasks(title) VALUES ('a')")).toEqual({ read: false, danger: null });
    expect(classifySql("SELECT 1; DELETE FROM tasks").danger).toContain("every row");
    expect(classifySql("DROP TABLE IF EXISTS \"my tasks\"").danger).toContain("deletes my tasks");
    expect(classifySql("DROP TABLE main.tasks").danger).toContain("deletes tasks");
    // A trigger body, including a CASE ... END inside it, is one statement.
    expect(classifySql("CREATE TRIGGER t AFTER INSERT ON tasks BEGIN UPDATE tasks SET done = CASE WHEN 1 THEN 1 END; END; SELECT 1").danger)
      .toBe("CREATE changes the database structure outside schema.ts.");
  });

  /** The bug this pins: a leading WITH made any statement read as a SELECT. */
  test("a leading WITH is classified by its main verb", () => {
    expect(classifySql("WITH x AS (SELECT 1) SELECT * FROM x")).toEqual({ read: true, danger: null });
    expect(classifySql("WITH x AS (SELECT 1) DELETE FROM tasks").danger).toContain("every row");
    expect(classifySql("WITH x AS (SELECT id FROM tasks WHERE done = 1) DELETE FROM tasks").danger).toContain("every row");
    expect(classifySql("WITH x AS (SELECT 1) DELETE FROM tasks WHERE id IN (SELECT * FROM x)")).toEqual({ read: false, danger: null });
    expect(classifySql("WITH RECURSIVE x(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM x WHERE n < 3) UPDATE tasks SET done = 1").danger).toContain("every row");
    expect(classifySql("WITH a AS (SELECT 1), b AS (SELECT 2) INSERT INTO tasks(title) SELECT 'x'")).toEqual({ read: false, danger: null });
  });

  test("a WHERE inside a subquery does not filter the outer statement", () => {
    expect(classifySql("UPDATE tasks SET done = (SELECT 1 FROM notes WHERE id = 1)").danger).toContain("every row");
    expect(classifySql("DELETE FROM tasks RETURNING id").danger).toContain("every row");
  });

  test("statements the console cannot read are writes, so the studio refreshes", () => {
    expect(classifySql("VACUUM").read).toBe(false);
    expect(classifySql("WITH x AS (SELECT 1)").read).toBe(false);
    expect(classifySql("BEGIN; COMMIT").read).toBe(false);
    expect(classifySql("SELECT replace(title, 'a', 'b') FROM tasks")).toEqual({ read: true, danger: null });
  });

  test("reads table names from a Drizzle model", () => {
    expect(modelTableNames('export const tasks = sqliteTable("tasks", {});\nconst n = sqliteTable(\'notes\', {})')).toEqual(["tasks", "notes"]);
  });
});
