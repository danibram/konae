import { describe, expect, test } from "bun:test";
import { planDrizzleDatabaseChange } from "./server";

const base = `import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const tasks = sqliteTable("tasks", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  title: text("title").notNull(),
});
`;

const withColumn = base.replace('title: text("title").notNull(),', 'title: text("title").notNull(),\n  dueAt: integer("due_at"),');
const renamedColumn = base.replace('title: text("title").notNull(),', 'label: text("label").notNull(),');
const otherTable = base.replace(/tasks/g, "notes");

describe("planning a migration", () => {
  test("derives the SQL for an added column", async () => {
    const plan = await planDrizzleDatabaseChange({ acceptedSource: base, candidateSource: withColumn });
    expect(plan.sql).toContain("ALTER TABLE");
    expect(plan.sql).toContain("due_at");
    expect(plan.destructive).toBe(false);
    expect(plan.fromHash).not.toBe(plan.toHash);
  }, 30_000);

  test("marks a dropped table as destructive", async () => {
    const plan = await planDrizzleDatabaseChange({ acceptedSource: withColumn, candidateSource: base });
    expect(plan.destructive).toBe(true);
  }, 30_000);

  test("refuses a model with no change to migrate", async () => {
    await expect(planDrizzleDatabaseChange({ acceptedSource: base, candidateSource: base })).rejects.toThrow(/no schema changes/i);
  }, 30_000);

  test("refuses a model that imports anything else", async () => {
    const hostile = `import { readFileSync } from "fs";\nexport const x = readFileSync;\n`;
    await expect(planDrizzleDatabaseChange({ acceptedSource: base, candidateSource: hostile }))
      .rejects.toThrow(/may only import from drizzle-orm/i);
  }, 30_000);
});

/**
 * Drizzle's differ resolves a rename by asking a human, and decides whether to
 * ask by looking for a terminal. A Kernel that found one would render a prompt
 * into a request nobody can answer and never reply, so these changes have to be
 * refused before the differ ever sees them.
 */
describe("changes that would need a human to disambiguate", () => {
  test("refuses a renamed column with an answer the owner can act on", async () => {
    const started = Date.now();
    await expect(planDrizzleDatabaseChange({ acceptedSource: base, candidateSource: renamedColumn }))
      .rejects.toThrow(/rename from a replacement/i);
    expect(Date.now() - started).toBeLessThan(20_000);
  }, 30_000);

  test("refuses a replaced table", async () => {
    await expect(planDrizzleDatabaseChange({ acceptedSource: base, candidateSource: otherTable }))
      .rejects.toThrow(/rename from a replacement/i);
  }, 30_000);
});

describe("evaluation is contained, not merely filtered", () => {
  /**
   * The static guard reads source text, so it cannot see what a model assembles
   * at runtime. This model gets past it. The point of the test is what happens
   * next: the derivation runs in its own process with no environment, so the
   * secret the Kernel holds is not there to be read.
   */
  test("a model that defeats the static guard still finds no Kernel secrets", async () => {
    const previous = process.env.LITTLEBOX_ADMIN_KEY;
    process.env.LITTLEBOX_ADMIN_KEY = "super-secret-admin-key";
    try {
      const sneaky = base.replace(
        'export const tasks',
        `const read = Function("return typeof pro" + "cess === 'undefined' ? 'absent' : (pro" + "cess.env.LITTLEBOX_ADMIN_KEY || 'no-key')");\n\nexport const tasks`,
      ).replace('title: text("title").notNull(),', 'title: text("title").notNull(),\n  stolen: text(read()),');

      const plan = await planDrizzleDatabaseChange({ acceptedSource: base, candidateSource: sneaky });
      // It ran — the guard did not stop it — and it came back empty-handed.
      expect(plan.sql).not.toContain("super-secret-admin-key");
      expect(plan.sql).toMatch(/no-key|absent/);
    } finally {
      if (previous === undefined) delete process.env.LITTLEBOX_ADMIN_KEY;
      else process.env.LITTLEBOX_ADMIN_KEY = previous;
    }
  }, 30_000);

  test("a model that never finishes is killed instead of hanging the Kernel", async () => {
    const spinning = `import { sqliteTable, text } from "drizzle-orm/sqlite-core";
const wait = Function("const end = Date.now() + 60000; while (Date.now() < end) {}");
wait();
export const slow = sqliteTable("slow", { id: text("id") });
`;
    const started = Date.now();
    await expect(planDrizzleDatabaseChange({ acceptedSource: base, candidateSource: spinning }))
      .rejects.toThrow(/could not derive/i);
    expect(Date.now() - started).toBeLessThan(40_000);
  }, 45_000);
});
