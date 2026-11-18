import { describe, expect, test } from "bun:test";
import { boundModelText, declaredIntent, redactDurableValue, redactText } from "./durable-trace";

describe("durable trace privacy and model boundary", () => {
  test("scrubs known credentials, credential fields, URL userinfo and invitation/verification values", () => {
    const result = redactDurableValue(Object.fromEntries([
      ["apiKey", "sk-real-credential-1234567890"],
      ["x-api-key", "also-secret"],
      ["clientSecret", "client-secret"],
      ["APIKey", "upper-secret"],
      ["X-API-KEY", "header-secret"],
      ["AUTHORIZATION", "Bearer owner-secret"],
      ["response", "key=sk-real-credential-1234567890 https://user@example.test/x?invitation_code=abc&keep=ok#verify-email=xyz lb_0123456789abcdef"],
      ["__proto__", { harmless: true }],
    ]), ["sk-real-credential-1234567890"]);
    expect(result).toEqual(Object.fromEntries([
      ["apiKey", "[redacted]"],
      ["x-api-key", "[redacted]"],
      ["clientSecret", "[redacted]"],
      ["APIKey", "[redacted]"],
      ["X-API-KEY", "[redacted]"],
      ["AUTHORIZATION", "[redacted]"],
      ["response", "key=[redacted credential] https://%5Bredacted%5D:%5Bredacted%5D@example.test/x?invitation_code=%5Bredacted%5D&keep=ok#[redacted] [redacted credential]"],
      ["__proto__", { harmless: true }],
    ]));
  });

  test("preserves untouched URLs and bounds the exact model-visible text", () => {
    expect(redactText("See https://example.test/path?a=1")).toBe("See https://example.test/path?a=1");
    const result = boundModelText({ output: "x".repeat(20_001), apiKey: "sk-real-credential-1234567890" });
    expect(result.truncated).toBe(true);
    expect(result.redacted).toBe(true);
    expect(result.text.length).toBeLessThan(20_100);
    expect(result.text).not.toContain("sk-real-credential");
    expect(declaredIntent("owner-message", undefined, "objective", 1)).toBeUndefined();
    expect(declaredIntent("owner-message", "entry-1", " objective ", 1)).toEqual({ origin: "owner-message", sourceEntryId: "entry-1", text: "objective", declaredAt: 1 });
  });
});
