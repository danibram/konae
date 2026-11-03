import { describe, expect, test } from "bun:test";
import cases from "../../kernel-rs/kernel/src/domain-cases.json";
import { checkHostname, domainRecordName } from "./domain-name";

// The Kernel runs the same file in kernel-rs/kernel/src/domain.rs.
describe("custom domain hostnames", () => {
  const rules = { suffix: cases.context.suffix, installationHosts: cases.context.installationHosts };
  test("the case file covers every rule", () => {
    expect(cases.cases.length).toBeGreaterThanOrEqual(20);
  });
  for (const item of cases.cases) {
    test(item.case, () => {
      const outcome = checkHostname(item.hostname, rules);
      if (item.error === null) expect(outcome).toEqual({ ok: true, hostname: item.hostnameOut! });
      else expect(outcome).toEqual({ ok: false, code: (item as { code?: string }).code ?? "the case names no code", message: item.error });
    });
  }
  test("the proof lives under _konae", () => {
    expect(domainRecordName("notes.example.org")).toBe("_konae.notes.example.org");
  });
});
