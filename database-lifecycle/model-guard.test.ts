import { expect, test } from "bun:test";
import { assertConstrainedModel } from "./model-guard";

test("admits column builders and rejects computed execution before evaluating a schema", () => {
  const valid = `import { sqliteTable, integer, text } from "drizzle-orm/sqlite-core";
export const users = sqliteTable("users", { id: integer("id").primaryKey({ autoIncrement: true }), name: text("name").notNull().default("User") });`;
  expect(() => assertConstrainedModel(valid, "schema.ts")).not.toThrow();
  const attacks = [
    `import { sqliteTable } from "drizzle-orm/sqlite-core"; export const exploit = ({}).constructor.constructor("return 1")();`,
    `import { sqliteTable } from "drizzle-orm/sqlite-core"; export const exploit = sqliteTable["constructor"]("return 1")();`,
    `import { sqliteTable } from "drizzle-orm/sqlite-core"; export const exploit = sqliteTable("x", { get value() { return 1; } });`,
    `import { sqliteTable } from "drizzle-orm/sqlite-core"; export const exploit = (() => { throw "executed"; })();`,
    `import { sqliteTable } from "drizzle-orm/sqlite-core"; export const exploit = sqliteTable("x", { ["__proto__"]: {} });`,
  ];
  for (const attack of attacks) expect(() => assertConstrainedModel(attack, "schema.ts")).toThrow("only permits Drizzle declarations");
});
