import { describe, expect, test } from "bun:test";
import type { DatabaseRunResult } from "../runtime/host";
import { createSchemaSnapshot } from "./schema-snapshot";

describe("createSchemaSnapshot", () => {
  test("returns executable idempotent DDL for user tables and dependent objects", async () => {
    const result: DatabaseRunResult = {
      columns: ["type", "sql"],
      rows: [
        { type: "table", sql: "CREATE TABLE notes (id INTEGER PRIMARY KEY)" },
        { type: "table", sql: "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY)" },
        { type: "index", sql: "CREATE INDEX notes_created ON notes(created_at)" },
        { type: "view", sql: "CREATE VIEW visible_notes AS SELECT id FROM notes" },
        { type: "trigger", sql: "CREATE TRIGGER note_audit AFTER INSERT ON notes BEGIN SELECT 1; END" },
        { type: "table", sql: null },
        { type: "table", sql: "   " },
      ],
      changes: 0,
      elapsedMs: 0,
      statement: "",
    };
    let receivedQuery = "";
    const snapshot = await createSchemaSnapshot({ runSql: async (sql) => { receivedQuery = sql; return result; } });

    expect(receivedQuery).toContain("sqlite_master");
    expect(receivedQuery).toContain("name NOT GLOB 'sqlite_*'");
    expect(snapshot).toBe([
      "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY);",
      "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY);",
      "CREATE INDEX IF NOT EXISTS notes_created ON notes(created_at);",
      "CREATE VIEW IF NOT EXISTS visible_notes AS SELECT id FROM notes;",
      "CREATE TRIGGER IF NOT EXISTS note_audit AFTER INSERT ON notes BEGIN SELECT 1; END;",
    ].join("\n\n") + "\n");
  });

  test("returns an empty source when the database has no user schema objects", async () => {
    const snapshot = await createSchemaSnapshot({
      runSql: async () => ({ columns: [], rows: [], changes: 0, elapsedMs: 0, statement: "" }),
    });
    expect(snapshot).toBe("");
  });
});
