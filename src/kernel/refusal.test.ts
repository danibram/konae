import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { hintOf, joinRefusals, ownerFixes, refusal, refusalCodes } from "./refusal";

const dir = path.join(import.meta.dir, "../../kernel-rs/src");

/** Every refusal case in every shape case file, as kernel-rs/src/refusal.rs reads them. */
function refusalCases(): { file: string; case: string; code?: string }[] {
  const found: { file: string; case: string; code?: string }[] = [];
  for (const file of readdirSync(dir).filter((name) => name.endsWith("-cases.json")).sort()) {
    const value = JSON.parse(readFileSync(path.join(dir, file), "utf8"));
    const cases: { case: string; error?: unknown; code?: string }[] = Array.isArray(value) ? value : value.cases;
    for (const item of cases) if (typeof item.error === "string") found.push({ file, case: item.case, code: item.code });
  }
  return found;
}

describe("refusal codes (ADR-0020)", () => {
  test("every refusal case names a code from the table", () => {
    const cases = refusalCases();
    expect(cases.length).toBeGreaterThan(300);
    const codes = new Set<string>(refusalCodes());
    for (const item of cases) expect(item.code && codes.has(item.code) ? item.code : `${item.file}: "${item.case}" names ${item.code ?? "no code"}`).toBe(item.code!);
  });

  test("every code in the table has a case", () => {
    const used = new Set(refusalCases().map((item) => item.code));
    expect(refusalCodes().filter((code) => !used.has(code))).toEqual([]);
  });

  test("every code the editor names is in the table", () => {
    const sources = ["src/kernel/actions.ts", "src/kernel/calls.ts", "src/kernel/route-declaration.ts", "src/kernel/sql-rules.ts", "src/agent/release-check.ts", "src/agent/gate.ts", "src/lib/domain-name.ts", "src/lib/carry-checks.ts"];
    const codes = new Set<string>(refusalCodes());
    let named = 0;
    for (const source of sources) {
      const text = readFileSync(path.join(import.meta.dir, "../..", source), "utf8");
      for (const match of text.matchAll(/(?:refusal|refuse|refused|hintOf)\("([a-z]+\.[a-z0-9.-]+)"/g)) {
        named += 1;
        expect(codes.has(match[1]) ? match[1] : `${source} names ${match[1]}`).toBe(match[1]);
      }
    }
    expect(named).toBeGreaterThan(100);
  });

  test("a joined refusal keeps the first code and every sentence", () => {
    const joined = joinRefusals([
      refusal("signature.header-invalid", "Action a.b has an invalid signature header."),
      refusal("signature.secret-invalid", "Action a.b names an invalid signature secret."),
    ]);
    expect(joined.toJSON()).toEqual({
      code: "signature.header-invalid",
      message: "Action a.b has an invalid signature header. Action a.b names an invalid signature secret.",
      hint: hintOf("signature.header-invalid"),
    });
  });

  test("the owner fixes a secret, an edit fixes the rest", () => {
    expect(ownerFixes("route.signature-secret-unset")).toBe(true);
    expect(hintOf("route.signature-secret-unset").tool).toBe("secret_set");
    expect(ownerFixes("query.limit-missing")).toBe(false);
    expect(ownerFixes("not.a-code")).toBe(false);
  });
});
