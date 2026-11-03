import { describe, expect, test } from "bun:test";
import { parseRouteSignature } from "../kernel/route-declaration";
import signaturePresets from "../../kernel-rs/kernel/src/signature-presets.json";
import { signDelivery } from "./provider-signature";
import { expectedOutcome, outcomeOf } from "../kernel/refusal-cases";

type Vector = { case: string; signature: unknown; secret: string; body: string; t?: number; id?: string; headers: Record<string, string> };

describe("a provider's signature", () => {
  test("matches the vectors the Kernel verifies, GitHub's and Svix's published examples among them", async () => {
    const vectors: Vector[] = await Bun.file(new URL("../../kernel-rs/kernel/src/provider-signature-cases.json", import.meta.url)).json();
    expect(vectors.length).toBeGreaterThanOrEqual(5);
    for (const vector of vectors) {
      const signature = parseRouteSignature("hooks.receive", "POST", vector.signature);
      expect(await signDelivery(signature, vector.secret, vector.body, { t: vector.t, id: vector.id }), vector.case).toEqual(vector.headers);
    }
  });

  test("every preset in the table signs a vector", async () => {
    const vectors: Vector[] = await Bun.file(new URL("../../kernel-rs/kernel/src/provider-signature-cases.json", import.meta.url)).json();
    for (const provider of Object.keys(signaturePresets)) {
      expect(vectors.some((vector) => (vector.signature as { provider?: string }).provider === provider), provider).toBe(true);
    }
  });

  test("a preset expands to its long form and a refusal names every wrong key, as the Kernel does", async () => {
    type Case = { case: string; method: "GET" | "POST"; signature: unknown; error?: string; code?: string; expands?: unknown };
    const cases: Case[] = await Bun.file(new URL("../../kernel-rs/kernel/src/signature-preset-cases.json", import.meta.url)).json();
    expect(cases.length).toBeGreaterThanOrEqual(15);
    for (const item of cases) {
      const parse = (value: unknown) => () => parseRouteSignature("hooks.receive", item.method, value);
      if (item.error !== undefined) {
        expect(outcomeOf(parse(item.signature)), item.case).toEqual(expectedOutcome({ error: item.error, code: item.code }));
      } else {
        expect(parse(item.signature)(), item.case).toEqual(parse(item.expands)());
      }
    }
  });

  test("a svix delivery gets a fresh id, and Konae's own scheme is not signed here", async () => {
    const svix = parseRouteSignature("hooks.receive", "POST", { scheme: "svix", secret: "resend_webhook" });
    const secret = `whsec_${btoa("svix-test-key-0123456789")}`;
    const one = await signDelivery(svix, secret, "{}", { t: 1 });
    const two = await signDelivery(svix, secret, "{}", { t: 1 });
    expect(one["svix-id"]).not.toBe(two["svix-id"]);
    await expect(signDelivery({ header: "X-Signature", secret: "orders_webhook" }, "secret-0123456789", "{}")).rejects.toThrow("signRoute");
  });
});
