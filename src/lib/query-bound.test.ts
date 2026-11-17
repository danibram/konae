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
    expect(unboundedQueryReason("SELECT id FROM tasks WHERE :limit IS NOT NULL")).toContain("reads rows");
    expect(unboundedQueryReason("SELECT id FROM tasks WHERE @limit > 0 OR $limit > 0")).toContain("reads rows");
    expect(unboundedQueryReason("SELECT id FROM tasks LIMIT :limit")).toBeNull();
    expect(unboundedQueryReason("SELECT choice, count(*) AS n FROM votes GROUP BY choice")).toContain("groups rows");
    expect(unboundedQueryReason("SELECT sum(x) OVER () FROM tasks")).toContain("OVER");
    expect(unboundedQueryReason("SELECT count(*) FROM tasks UNION ALL SELECT count(*) FROM notes")).toContain("combines");
  });
});

test("recognizes totals inside COALESCE without exempting row expressions", () => {
  expect(unboundedQueryReason("SELECT COALESCE(SUM(amount), 0) AS total FROM payments")).toBeNull();
  expect(unboundedQueryReason("SELECT IFNULL(COALESCE(SUM(amount), NULL), 0) AS total FROM payments")).toBeNull();
  for (const sql of ["SELECT COALESCE(amount, 0) FROM payments", "SELECT COALESCE(SUM(amount), amount) FROM payments", "SELECT COALESCE((SELECT SUM(amount) FROM payments), 0) FROM payments", "SELECT MIN(amount, 0) FROM payments"]) {
    expect(unboundedQueryReason(sql)).toContain("reads rows");
  }
});
