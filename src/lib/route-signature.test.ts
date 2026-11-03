import { describe, expect, test } from "bun:test";
import { canonicalQuery, routeNonce, signRoute } from "./route-signature";

type Vector = { case: string; secret: string; t: number; n: string; method: "GET" | "POST"; path: string; query: string; body: string; canonicalQuery: string; header: string };

describe("route signatures", () => {
  // kernel-rs/kernel/src/http.rs checks the same vectors against the Kernel's signer.
  test("sign as the Kernel verifies", async () => {
    const vectors: Vector[] = await Bun.file(new URL("../../kernel-rs/kernel/src/signature-cases.json", import.meta.url)).json();
    expect(vectors.length).toBeGreaterThanOrEqual(3);
    for (const vector of vectors) {
      expect(canonicalQuery(vector.query), vector.case).toBe(vector.canonicalQuery);
      expect(await signRoute(vector.secret, vector, vector.t, vector.n), vector.case).toBe(vector.header);
    }
  });

  test("a query signs the same in any order, and two nonces in one second differ", async () => {
    const at = 1_800_000_000;
    const one = await signRoute("feed-secret-0123456", { method: "GET", path: "/api/events", query: "a=1&b=2" }, at, "x");
    const other = await signRoute("feed-secret-0123456", { method: "GET", path: "/api/events", query: "b=2&a=1" }, at, "x");
    expect(one).toBe(other);
    expect(await signRoute("feed-secret-0123456", { method: "GET", path: "/api/events", query: "a=1&b=2" }, at, "y")).not.toBe(one);
    expect(routeNonce()).toMatch(/^[A-Za-z0-9_-]{22}$/);
  });
});
