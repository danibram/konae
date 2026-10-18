import { describe, expect, test } from "bun:test";
import initSqlJs from "sql.js";
import { acceptedModel, deployedModel, plannedStatements, schemaTokens } from "./model-rules";

type Case = {
  case: string;
  rule: "accepted" | "deployed" | "planned";
  model?: { source: string; acceptedSource: string; revision: string };
  modelSchema?: string[];
  databaseSchema?: string[];
  statements?: string[];
  error: string | null;
  code?: string;
};

const cases: Case[] = await Bun.file(new URL("../../kernel-rs/src/database-model-cases.json", import.meta.url)).json();
const SQL = await initSqlJs();

function tokensOf(statements: string[]) {
  const db = new SQL.Database();
  try {
    for (const sql of statements) db.run(sql);
    return schemaTokens(db);
  } finally { db.close(); }
}

describe("database model rules", () => {
  // kernel-rs/src/model_rules.rs runs the same cases against the Kernel.
  test("every case in kernel-rs/src/database-model-cases.json gets the Kernel's answer", () => {
    expect(cases.length).toBeGreaterThanOrEqual(6);
    for (const item of cases) {
      const outcome = item.rule === "accepted" ? acceptedModel(item.model!)
        : item.rule === "deployed" ? deployedModel(Bun.deepEquals(tokensOf(item.modelSchema!), tokensOf(item.databaseSchema!)))
          : plannedStatements(item.statements!);
      expect(outcome ? { code: outcome.code, message: outcome.message } : null, item.case).toEqual(item.error === null ? null : { code: item.code as never, message: item.error });
    }
  });

  test("the tokens read a column added by ALTER TABLE in the model's place, and leave CHECKs out", () => {
    const [[kind, name, tokens]] = tokensOf(["CREATE TABLE notes(id INTEGER PRIMARY KEY AUTOINCREMENT, CONSTRAINT short CHECK (length(id) < 9))", "ALTER TABLE notes ADD body TEXT DEFAULT TRUE NOT NULL"]);
    expect([kind, name]).toEqual(["table", "notes"]);
    expect(tokens.join(" ")).toBe("create table notes ( body text not null default 1 , id integer primary key autoincrement )");
  });
});
