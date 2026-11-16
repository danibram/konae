import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { starterProject } from "../lib/starter";
import { createCapsuleValidator } from "./gate";
import { schemaTransformsRefusal } from "./schema-transforms";
import initSqlJs from "sql.js";

const validEmpty = JSON.stringify({ version: 1, transitions: [] });
const validTransition = JSON.stringify({
  version: 1,
  transitions: [{ id: "seed.rows", predecessor: null, before: `sha256:${"a".repeat(64)}`, after: `sha256:${"b".repeat(64)}`, maxRows: 20, statements: ["INSERT INTO tasks (title) VALUES ('sqlite_schema');"] }],
});
const schemaTransformCases = JSON.parse(readFileSync(new URL("../../kernel-rs/kernel/src/schema-transform-cases.json", import.meta.url), "utf8")) as {
  cases: { case: string; scope: "static" | "runtime"; source: string; error: string | null; code: string | null }[];
};

test("a missing transform file remains optional and a valid definition passes the static Gate", async () => {
  const validator = createCapsuleValidator(() => initSqlJs());
  const withoutTransforms = await validator.validate(starterProject.files);
  expect(withoutTransforms.ok).toBe(true);
  expect(withoutTransforms.warnings).toBeUndefined();
  expect(schemaTransformsRefusal(validEmpty)).toBeNull();
  expect(schemaTransformsRefusal(validTransition)).toBeNull();
  const withTransforms = await validator.validate({ ...starterProject.files, "schema-transforms.json": validTransition });
  expect(withTransforms.ok).toBe(true);
  expect(withTransforms.warnings).toContainEqual(expect.objectContaining({
    stage: "schema",
    message: expect.stringContaining("applied history, verify the database schema fingerprint, measure affected row counts, or enforce runtime constraints"),
  }));
  const emptyTransforms = await validator.validate({ ...starterProject.files, "schema-transforms.json": validEmpty });
  expect(emptyTransforms.ok).toBe(true);
  expect(emptyTransforms.warnings).toBeUndefined();
});

test("the Gate matches every shared static schema-transform case and skips runtime evidence", async () => {
  const staticCases = schemaTransformCases.cases.filter((item) => item.scope === "static");
  expect(staticCases.length).toBeGreaterThan(0);
  const validator = createCapsuleValidator(() => initSqlJs());
  for (const item of staticCases) {
    const result = schemaTransformsRefusal(item.source);
    if (item.error === null) expect(result, item.case).toBeNull();
    else expect(result, item.case).toMatchObject({ message: item.error, code: item.code });
    const validation = await validator.validate({ ...starterProject.files, "schema-transforms.json": item.source });
    if (item.error === null) expect(validation, item.case).toMatchObject({ ok: true });
    else expect(validation, item.case).toMatchObject({ ok: false, errors: [{ stage: "schema", message: item.error, code: item.code }] });
  }
  expect(schemaTransformCases.cases.some((item) => item.scope === "runtime")).toBe(true);
});

test("the Gate refuses a transformation document above the Kernel byte limit before parsing", () => {
  const oversized = " ".repeat(20 * 1024 * 1024 + 1);
  expect(schemaTransformsRefusal(oversized)).toMatchObject({
    code: "schema-transform.document-invalid",
    message: "schema-transforms.json must be at most 20971520 bytes.",
  });
});
