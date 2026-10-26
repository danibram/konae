import { describe, expect, test } from "bun:test";
import initSqlJs, { type SqlJsStatic } from "sql.js";
import cases from "../../kernel-rs/src/root-relative-cases.json";
import { createCapsuleValidator } from "../agent/gate";
import { starterProject } from "./starter";
import { assembleStaticArtifact, rootRelativeWarnings } from "./static-artifact.js";

// The Kernel would run the same file from kernel-rs/src/root-relative-cases.json.
// No Kernel check exists yet, so only the TypeScript side runs it.
describe("root-relative URL warnings", () => {
  test("the case file covers the rules", () => {
    expect(cases.cases.length).toBeGreaterThanOrEqual(20);
  });
  for (const item of cases.cases) {
    test(item.case, () => {
      expect(rootRelativeWarnings(item.files).map((warning) => warning.message)).toEqual(item.warnings);
    });
  }
});

describe("root-relative URL warnings in the build and the Gate", () => {
  test("the build hands each warning to input.warn and the artifact keeps its shape", async () => {
    const files: Record<string, string> = { "index.html": "<a href=\"/about\">About</a>" };
    const warned: string[] = [];
    const artifact = await assembleStaticArtifact({
      paths: Object.keys(files),
      read: async (path) => files[path],
      compile: async () => ({ styles: "", script: "" }),
      bundle: async () => ({ script: "", styles: "" }),
      runtime: "",
      warn: (message) => warned.push(message),
    });
    expect(warned).toEqual(["index.html links to /about, which leaves the app's path address. Write about instead."]);
    expect(Object.keys(artifact).sort()).toEqual(["entrypoint", "files", "version"]);
  });

  test("the Gate passes with a warning in the actions stage", async () => {
    let sql: Promise<SqlJsStatic> | undefined;
    const validator = createCapsuleValidator(() => (sql ??= initSqlJs()));
    const result = await validator.validate({
      ...starterProject.files,
      "pages/menu.html": "<a href=\"/about\">About</a>",
    });
    expect(result.ok).toBe(true);
    expect(result.warnings).toContainEqual({
      stage: "actions",
      message: "pages/menu.html links to /about, which leaves the app's path address. Write about instead.",
    });
  });
});
