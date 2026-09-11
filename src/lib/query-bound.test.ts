import { describe, expect, test } from "bun:test";
import { unboundedQueryReason } from "./query-bound";

describe("query bound", () => {
  test("accepts a top-level LIMIT, a total, and a query that reads no table", () => {
    expect(unboundedQueryReason("SELECT id, title FROM tasks ORDER BY created_at DESC LIMIT 100")).toBeNull();
    expect(unboundedQueryReason("SELECT count(*) AS n FROM waitlist")).toBeNull();
    expect(unboundedQueryReason("SELECT COUNT(*) AS total, sum(amount) s FROM payments")).toBeNull();
    expect(unboundedQueryReason("SELECT 1")).toBeNull();
    expect(unboundedQueryReason("SELECT datetime('now')")).toBeNull();
  });

  test("refuses a row query, a grouped query, a window and a compound query", () => {
    expect(unboundedQueryReason("SELECT id, title FROM tasks")).toContain("reads rows");
    expect(unboundedQueryReason("SELECT id FROM tasks WHERE title = 'LIMIT 5'")).toContain("reads rows");
    expect(unboundedQueryReason("-- LIMIT 10\nSELECT id FROM tasks")).toContain("reads rows");
    expect(unboundedQueryReason("SELECT id FROM (SELECT id FROM tasks LIMIT 1)")).toContain("reads rows");
    expect(unboundedQueryReason("SELECT choice, count(*) AS n FROM votes GROUP BY choice")).toContain("groups rows");
    expect(unboundedQueryReason("SELECT sum(x) OVER () FROM tasks")).toContain("OVER");
    expect(unboundedQueryReason("SELECT count(*) FROM tasks UNION ALL SELECT count(*) FROM notes")).toContain("combines");
  });
});
