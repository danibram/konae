import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import path from "node:path";
import { sourceRevisionConflict } from "./release";

describe("source revision refusal", () => {
  test("matches the Kernel's shared sentence and code", () => {
    const fixture = JSON.parse(readFileSync(path.join(import.meta.dir, "../../kernel-rs/kernel/src/source-revision-cases.json"), "utf8"));
    for (const item of fixture.cases) {
      const refusal = sourceRevisionConflict(item.observed);
      expect({ code: refusal.code, message: refusal.message }).toEqual({ code: item.code, message: item.error });
    }
  });
});
