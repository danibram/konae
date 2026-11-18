import { describe, expect, test } from "bun:test";
import { boundModelText, declaredIntent, durableOwnerMessage, redactDurableValue, redactText } from "./durable-trace";

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

  test("redacts a pasted fixture key from source and tool argument evidence", () => {
    const pasted = "fixture-key-pasted-into-capsule-source-2026";
    const result = redactDurableValue({ source: `const key = '${pasted}';`, toolArguments: { content: `write this ${pasted}` } }, [pasted]);
    expect(JSON.stringify(result)).not.toContain(pasted);
    expect(result).toMatchObject({ source: "const key = '[redacted credential]';", toolArguments: { content: "write this [redacted credential]" } });
  });

  test("preserves untouched URLs and bounds the exact model-visible text", () => {
    expect(redactText("See https://example.test/path?a=1")).toBe("See https://example.test/path?a=1");
    const providerError = redactText("Provider refused request: https://api.example.test/error?access_token=synthetic-private-access&refresh_token=synthetic-private-refresh&reason=denied");
    expect(providerError).toContain("Provider refused request:");
    expect(providerError).toContain("reason=denied");
    expect(providerError).not.toContain("synthetic-private-access");
    expect(providerError).not.toContain("synthetic-private-refresh");
    const result = boundModelText({ output: "x".repeat(20_001), apiKey: "sk-real-credential-1234567890" });
    expect(result.truncated).toBe(true);
    expect(result.redacted).toBe(true);
    expect(result.text.length).toBeLessThan(20_100);
    expect(result.text).not.toContain("sk-real-credential");
    expect(declaredIntent("owner-message", undefined, "objective", 1)).toBeUndefined();
    expect(declaredIntent("owner-message", "entry-1", " objective ", 1)).toEqual({ origin: "owner-message", sourceEntryId: "entry-1", text: "objective", declaredAt: 1 });
  });

  test("an explicit Objective line adds redacted self-sourced metadata without rewriting the prompt", () => {
    const prompt = "Objective: Keep the app portable with sk-proj-abcdefgh\n\nBuild a checklist.";
    const prepared = durableOwnerMessage(prompt, 1_700_000_000_123, ["owner-secret-value"]);
    expect(prepared.content).toBe("Objective: Keep the app portable with [redacted credential]\n\nBuild a checklist.");
    expect(prepared.entry).toEqual({
      kind: "konae.owner-message",
      data: { intent: { origin: "owner-message", sourceEntryId: "$self", text: "Keep the app portable with [redacted credential]", declaredAt: 1_700_000_000_123 } },
    });
  });

  test("ordinary owner input has no inferred declaration", () => {
    expect(durableOwnerMessage("Build a checklist.", 1_700_000_000_123)).toEqual({
      content: "Build a checklist.",
      entry: { kind: "konae.owner-message" },
    });
    expect(durableOwnerMessage("A note\nObjective: hidden as a declaration", 1)).toEqual({
      content: "A note\nObjective: hidden as a declaration",
      entry: { kind: "konae.owner-message" },
    });
  });

  test("redacts credential assignments in the declaration and admitted prompt and bounds the metadata", () => {
    const credential = "fixture-pasted-password-value";
    const tail = "\n\nUse the current checklist.";
    const prompt = `Objective: Password=${credential}; preserve the checklist ${"x".repeat(600)}${tail}`;
    const prepared = durableOwnerMessage(prompt, 2);
    const prefix = "Password=[redacted]; preserve the checklist ";
    expect(prepared.entry.data?.intent.text).toBe(`${prefix}${"x".repeat(500 - prefix.length)}`);
    expect(prepared.content).toBe(`Objective: ${prefix}${"x".repeat(600)}${tail}`);
  });
});
