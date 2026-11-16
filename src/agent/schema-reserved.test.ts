import { expect, test } from "bun:test";
import initSqlJs from "sql.js";
import { starterProject } from "../lib/starter";
import cases from "../../kernel-rs/kernel/src/schema-reserved-cases.json";
import { createCapsuleValidator } from "./gate";

test("the Gate refuses reserved schema names with the Kernel's words", async () => {
  const validator = createCapsuleValidator(() => initSqlJs());
  for (const item of cases) {
    const result = await validator.validate({ ...starterProject.files, "schema.sql": item.schema });
    expect(result, item.case).toMatchObject({ ok: false, errors: [{ stage: "schema", code: item.code, message: item.error }] });
  }
});
