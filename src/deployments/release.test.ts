import { expect, test } from "bun:test";
import { describeSchemaChange } from "./release";

test("schema transformation plans show row counts, limits and SQL without calling data changes losses", () => {
  expect(describeSchemaChange({
    statements: ["CREATE TABLE tasks (id INTEGER PRIMARY KEY)"],
    losses: [],
    confirmation: null,
    transformations: [{
      id: "seed.tasks",
      definitionSha256: "a".repeat(64),
      before: `sha256:${"b".repeat(64)}`,
      after: `sha256:${"c".repeat(64)}`,
      statements: ["INSERT INTO tasks (id) VALUES (1)"],
      maxRows: 100,
      affectedRows: 0,
      baselineSkipped: true,
    }],
  })).toEqual([
    "The Kernel applies these data transformations:",
    "  seed.tasks changes 0 rows (limit 100). SQL is skipped for the fresh database.",
    "    INSERT INTO tasks (id) VALUES (1);",
    "The Kernel runs these schema statements in one transaction:",
    "  CREATE TABLE tasks (id INTEGER PRIMARY KEY);",
  ]);
});
