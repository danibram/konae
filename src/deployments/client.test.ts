import { afterEach, describe, expect, test } from "bun:test";
import { HostedDatabaseClient } from "./client";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe("HostedDatabaseClient portable export", () => {
  test("requests the deployment-level route and decodes all state files", async () => {
    let requestUrl = "";
    let requestHeaders: HeadersInit | undefined;
    globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
      requestUrl = String(input);
      requestHeaders = init?.headers;
      return new Response(JSON.stringify({
        result: {
          initialDatabase: "AAH/",
          initialIdentity: "AQID",
          initialFiles: "BAUG",
        },
      }), { status: 200, headers: { "content-type": "application/json" } });
    }) as typeof fetch;

    const state = await new HostedDatabaseClient("deploy/a").exportPortableState();

    expect(requestUrl).toBe("/_littlebox/admin/deployments/deploy%2Fa/portable-state");
    expect(new Headers(requestHeaders).get("x-littlebox-admin-key")).toBeTruthy();
    expect(state.initialDatabase).toEqual(Uint8Array.from([0, 1, 255]));
    expect(state.initialIdentity).toEqual(Uint8Array.from([1, 2, 3]));
    expect(state.initialFiles).toEqual(Uint8Array.from([4, 5, 6]));
  });

  test("surfaces a portable export error without returning partial state", async () => {
    globalThis.fetch = (async () => new Response(JSON.stringify({ error: "Export unavailable." }), { status: 503 })) as typeof fetch;
    await expect(new HostedDatabaseClient("deploy_1").exportPortableState()).rejects.toThrow("Export unavailable.");
  });
});
