import { describe, expect, test } from "bun:test";
import initSqlJs from "sql.js";
import { createCapsuleValidator } from "../agent/gate";
import { starterProject } from "../lib/starter";
import { parseServerActionSource } from "./actions";
import { expectedOutcome, outcomeOf } from "./refusal-cases";

type SqlRuleCase = { case: string; actions?: Record<string, unknown>; schema?: string; error: string | null; code?: string };

describe("SQL rules", () => {
  // kernel-rs/src/sql_rules.rs runs the same cases against the Kernel.
  test("refuse the same statements and schemas as the Kernel, with the same messages", async () => {
    const cases: SqlRuleCase[] = await Bun.file(new URL("../../kernel-rs/src/sql-rule-cases.json", import.meta.url)).json();
    expect(cases.length).toBeGreaterThanOrEqual(12);
    const gate = createCapsuleValidator(() => initSqlJs());
    for (const item of cases) {
      if (item.schema !== undefined) {
        const result = await gate.validate({ ...starterProject.files, "schema.sql": item.schema, "actions.json": JSON.stringify({ version: 1, actions: {} }) });
        if (item.error === null) expect(result.errors, item.case).toEqual([]);
        else expect({ code: result.errors[0]?.code, message: result.errors[0]?.message }, item.case).toEqual(expectedOutcome(item));
        continue;
      }
      expect(outcomeOf(() => parseServerActionSource(JSON.stringify({ version: 1, actions: item.actions }))), item.case).toEqual(expectedOutcome(item));
    }
  });
});
