import { describe, expect, test } from "bun:test";
import { parseRouteSignature } from "../kernel/route-declaration";
import { signDelivery } from "./provider-signature";

type Vector = { case: string; signature: unknown; secret: string; body: string; t?: number; id?: string; headers: Record<string, string> };

describe("a provider's signature", () => {
  test("matches the vectors the Kernel verifies, GitHub's and Svix's published examples among them", async () => {
    const vectors: Vector[] = await Bun.file(new URL("../../kernel-rs/src/provider-signature-cases.json", import.meta.url)).json();
    expect(vectors.length).toBeGreaterThanOrEqual(5);
    for (const vector of vectors) {
      const signature = parseRouteSignature("hooks.receive", "POST", vector.signature);
      expect(await signDelivery(signature, vector.secret, vector.body, { t: vector.t, id: vector.id }), vector.case).toEqual(vector.headers);
    }
  });

  test("a svix delivery gets a fresh id, and Littlebox's own scheme is not signed here", async () => {
    const svix = parseRouteSignature("hooks.receive", "POST", { scheme: "svix", secret: "resend_webhook" });
    const secret = `whsec_${btoa("svix-test-key-0123456789")}`;
    const one = await signDelivery(svix, secret, "{}", { t: 1 });
    const two = await signDelivery(svix, secret, "{}", { t: 1 });
    expect(one["svix-id"]).not.toBe(two["svix-id"]);
    await expect(signDelivery({ header: "X-Signature", secret: "orders_webhook" }, "secret-0123456789", "{}")).rejects.toThrow("signRoute");
  });
});
