import { describe, expect, test } from "bun:test";
import {
  assertRuntimeDatabaseWithinLimits,
  assertRuntimeResultWithinLimits,
  assertRuntimeSql,
  runtimeLimits,
  validateRuntimeParams,
} from "./security";

describe("runtime SQL policy", () => {
  test("allows data queries and mutations used by mini-apps", () => {
    expect(() => assertRuntimeSql("query", "-- list tasks\nSELECT * FROM tasks")).not.toThrow();
    expect(() => assertRuntimeSql("execute", "INSERT INTO tasks(title) VALUES (?)")).not.toThrow();
    expect(() => assertRuntimeSql("execute", "UPDATE tasks SET done = 1 WHERE id = ?")).not.toThrow();
    expect(() => assertRuntimeSql("execute", "DELETE FROM tasks WHERE id = ?")).not.toThrow();
  });

  test("keeps structural and advanced SQL out of the public runtime", () => {
    expect(() => assertRuntimeSql("query", "PRAGMA table_info(tasks)")).toThrow();
    expect(() => assertRuntimeSql("query", "WITH rows AS (SELECT 1) SELECT * FROM rows")).toThrow();
    expect(() => assertRuntimeSql("execute", "DROP TABLE tasks")).toThrow();
    expect(() => assertRuntimeSql("execute", "BEGIN TRANSACTION")).toThrow();
  });
});

describe("runtime resource limits", () => {
  test("validates parameter types and total size", () => {
    expect(validateRuntimeParams(["task", 1, null])).toEqual(["task", 1, null]);
    expect(() => validateRuntimeParams([true])).toThrow();
    expect(() => validateRuntimeParams(Array.from({ length: runtimeLimits.parameters + 1 }, () => 1))).toThrow();
    expect(() => validateRuntimeParams(["x".repeat(runtimeLimits.parameterBytes + 1)])).toThrow();
  });

  test("caps result rows, result bytes and database bytes", () => {
    const tooManyRows = Array.from({ length: runtimeLimits.resultRows + 1 }, () => ({ id: 1 }));
    expect(() => assertRuntimeResultWithinLimits(tooManyRows)).toThrow();
    expect(() => assertRuntimeResultWithinLimits([{ value: "x".repeat(runtimeLimits.resultBytes + 1) }])).toThrow();
    expect(() => assertRuntimeDatabaseWithinLimits(runtimeLimits.databaseBytes + 1)).toThrow();
  });
});
