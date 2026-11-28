import { describe, expect, test } from "bun:test";
import cases from "../../kernel-rs/kernel/src/file-intake-cases.json";
import initSqlJs from "sql.js";
import { createCapsuleValidator } from "../agent/gate";
import { validateFileIntake } from "./file-intake";
import ftpsCases from "../../kernel-rs/kernel/src/ftps-cases.json";
import { validateFtpsDeclaration } from "./ftps";

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

test("the Validation Gate refuses action effects on a File intake consumer", async () => {
  const gate = createCapsuleValidator(() => initSqlJs());
  const result = await gate.validate({
    "index.html": "<!doctype html><title>Intake</title>",
    "schema.sql": "CREATE TABLE IF NOT EXISTS received (id TEXT PRIMARY KEY);",
    "actions.json": JSON.stringify({ version: 1, actions: {
      "files.record": {
        kind: "mutation", access: "anonymous", statement: "DELETE FROM received WHERE id = ?1",
        bindings: [{ source: "input", key: "versionId", type: "string" }], guard: "No file received.",
      },
    } }),
    "file-intake.json": JSON.stringify({
      version: 1, sources: [{ id: "camera", version: 1 }],
      consumers: [{ id: "index", source: "camera", action: "files.record" }],
    }),
  }, { state: "absent" }, { modules: { actions: { on: true }, "file-intake": { on: true } } });
  expect(result.errors.map(({ message }) => message)).toContain(
    "File intake consumer index action cannot declare guard, issue, notify or signal.",
  );
});

describe("FTPS declaration shared cases", () => {
  for (const item of ftpsCases) {
    test(item.case, () => {
      if (item.valid) expect(() => validateFtpsDeclaration(item.value)).not.toThrow();
      else expect(() => validateFtpsDeclaration(item.value)).toThrow(item.error);
    });
  }
});

test("the Validation Gate requires auth and files only for member destinations", async () => {
  const gate = createCapsuleValidator(() => initSqlJs());
  const files = { "index.html": "<!doctype html><title>FTPS</title>", "schema.sql": "", "ftps.json": JSON.stringify({ version: 1, destinations: { photos: { kind: "member" } } }) };
  const off = await gate.validate(files, { state: "absent" }, { modules: { auth: { on: true }, files: { on: false } } });
  expect(off.errors[0]?.message).toContain("Module files is off.");
  const authOff = await gate.validate(files, { state: "absent" }, { modules: { auth: { on: false }, files: { on: true } } });
  expect(authOff.errors[0]?.message).toContain("Module files requires auth, which is off.");
  const device = await gate.validate({ ...files, "ftps.json": JSON.stringify({ version: 1, destinations: { meters: { kind: "device", folder: "readings" } } }) }, { state: "absent" }, { modules: { auth: { on: false }, files: { on: false }, actions: { on: false }, "file-intake": { on: false } } });
  expect(device.errors.some((error) => error.message.includes("Module files is off.") || error.message.includes("Module auth is off."))).toBe(false);
});

test("the Validation Gate applies every shared FTPS refusal", async () => {
  const gate = createCapsuleValidator(() => initSqlJs());
  for (const item of ftpsCases) {
    const result = await gate.validate({
      "index.html": "<!doctype html><title>FTPS</title>",
      "schema.sql": "",
      "ftps.json": JSON.stringify(item.value),
    }, { state: "absent" }, { modules: { auth: { on: true }, files: { on: true } } });
    expect(result.errors.some((error) => error.message.includes("app/ftps.json") || error.message.includes("FTPS destination")), item.case).toBe(!item.valid);
  }
});
