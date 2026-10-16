import { describe, expect, test } from "bun:test";
import initSqlJs from "sql.js";
import { carryChecks } from "./carry-checks";

type Case = { case: string; tables: Record<string, string>; statements: string[]; expected: string[] | null; error: string | null };

const cases: Case[] = await Bun.file(new URL("../../kernel-rs/src/check-carry-cases.json", import.meta.url)).json();

describe("carryChecks", () => {
  // kernel-rs/src/checks.rs runs the same cases against the Kernel.
  test("every case in kernel-rs/src/check-carry-cases.json gets the Kernel's answer", () => {
    expect(cases.length).toBeGreaterThanOrEqual(11);
    for (const item of cases) {
      const outcome = carryChecks(item.statements, (name) => item.tables[name] ?? null);
      expect(outcome, item.case).toEqual(item.error === null ? { statements: item.expected! } : { error: item.error });
    }
  });

  test("the carried statements run on SQLite and its CHECKs hold afterwards", async () => {
    const SQL = await initSqlJs();
    for (const item of cases.filter((candidate) => candidate.expected && Object.keys(candidate.tables).length > 0)) {
      const db = new SQL.Database();
      try {
        for (const sql of Object.values(item.tables)) db.run(sql);
        db.run("BEGIN");
        for (const statement of item.expected!) db.run(statement);
        db.run("COMMIT");
        const habits = db.exec("SELECT sql FROM sqlite_master WHERE name = 'habits'")[0]?.values[0]?.[0];
        if (typeof habits === "string" && habits.includes("CHECK (state IN")) {
          expect(() => db.run("INSERT INTO habits(name, state) VALUES('walk', 'maybe')"), item.case).toThrow("CHECK constraint failed");
        }
      } finally { db.close(); }
    }
  });
});
