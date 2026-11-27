import { describe, expect, test } from "bun:test";
import cases from "../../kernel-rs/kernel/src/file-intake-cases.json";
import initSqlJs from "sql.js";
import { createCapsuleValidator } from "../agent/gate";
import { validateFileIntake } from "./file-intake";

describe("file intake declaration shared cases", () => {
  for (const item of cases) {
    test(item.case, () => {
      if (item.valid) expect(() => validateFileIntake(item.value)).not.toThrow();
      else expect(() => validateFileIntake(item.value)).toThrow();
    });
  }
});

test("the Validation Gate uses the shared declaration cases", async () => {
  const gate = createCapsuleValidator(() => initSqlJs());
  for (const item of cases) {
    const result = await gate.validate({
      "index.html": "<!doctype html><title>Intake</title>",
      "schema.sql": "",
      "actions.json": JSON.stringify({ version: 1, actions: {} }),
      "file-intake.json": JSON.stringify(item.value),
    }, { state: "absent" }, { modules: { actions: { on: true }, "file-intake": { on: true } } });
    expect(result.errors.some((error) => error.message.includes("file-intake.json") || error.message.includes("File intake")), item.case).toBe(!item.valid);
  }
});
