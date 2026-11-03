import { describe, expect, test } from "bun:test";
import cases from "../../kernel-rs/kernel/src/path-cases.json";
import { checkPathName } from "./path-name";

// The Kernel runs the same file in kernel-rs/kernel/src/path_name.rs.
describe("path address names", () => {
  test("the case file covers every rule", () => {
    expect(cases.cases.length).toBeGreaterThanOrEqual(30);
  });
  for (const item of cases.cases) {
    test(item.case, () => {
      const outcome = checkPathName(item.name);
      if (item.error === null) expect(outcome).toEqual({ ok: true, name: item.nameOut! });
      else expect(outcome).toEqual({ ok: false, code: (item as { code?: string }).code ?? "the case names no code", message: item.error });
    });
  }
});
