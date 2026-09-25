import { describe, expect, test } from "bun:test";
import { McpClient } from "./client";

/** A fake MCP endpoint that records each tool call and answers a transfer. */
function endpoint() {
  const calls: { name: string; arguments: Record<string, unknown> }[] = [];
  const fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
    const request = JSON.parse(String(init?.body));
    const reply = (result: unknown) => new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }), { headers: { "content-type": "application/json" } });
    if (request.method === "initialize") return reply({ protocolVersion: "2025-11-25" });
    if (request.method !== "tools/call") return new Response(null, { status: 202 });
    calls.push(request.params);
    const result = request.params.name === "transfer_begin" ? { transferId: "t1", size: 3, sha256: "", chunkBytes: 2 } : {};
    return reply({ structuredContent: { result } });
  }) as typeof globalThis.fetch;
  return { calls, client: new McpClient("http://kernel.test/mcp", { fetch }) };
}

describe("McpClient.upload", () => {
  test("names a package component in transfer_begin when asked", async () => {
    const { calls, client } = endpoint();
    await client.upload(new Uint8Array([1, 2, 3]), "app_1", "identity");
    expect(calls.map((call) => call.name)).toEqual(["transfer_begin", "transfer_write", "transfer_write", "transfer_complete"]);
    expect(calls[0].arguments).toMatchObject({ projectId: "app_1", size: 3, component: "identity" });
  });

  test("leaves component out of an ordinary upload", async () => {
    const { calls, client } = endpoint();
    await client.upload(new Uint8Array([1, 2, 3]), "app_1");
    expect(calls[0].arguments).not.toHaveProperty("component");
  });
});
