import { describe, expect, test } from "bun:test";
import { McpClient } from "./client";

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
  test("binds agent MCP calls to the server-fenced execution identity", async () => {
    let headers: Headers | undefined;
    const fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
      const request = JSON.parse(String(init?.body));
      headers = new Headers(init?.headers);
      if (request.method === "notifications/initialized") return new Response(null, { status: 202 });
      const result = request.method === "initialize" ? { protocolVersion: "2025-11-25" } : { structuredContent: { result: { ok: true } } };
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }));
    }) as typeof globalThis.fetch;
    const client = new McpClient("/mcp", { ownerId: "fixture-owner", fetch });
    await client.callForAgent("module_set", { projectId: "project-a", module: "files", on: true }, { projectId: "project-a", conversationId: "conversation-a", executorId: "device-a", generation: 8, callId: "tool-call-1", observesCallId: "original-write-1" });
    expect(headers?.get("x-konae-owner-id")).toBe("fixture-owner");
    expect(headers?.get("x-konae-execution-project")).toBe("project-a");
    expect(headers?.get("x-konae-execution-conversation")).toBe("conversation-a");
    expect(headers?.get("x-konae-execution-id")).toBe("device-a");
    expect(headers?.get("x-konae-execution-generation")).toBe("8");
    expect(headers?.get("x-konae-execution-call")).toBe("tool-call-1");
    expect(headers?.get("x-konae-execution-observes")).toBe("original-write-1");
  });

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

  test("cancels a transfer when the signal aborts during transfer_begin", async () => {
    const calls: string[] = [];
    let enter!: () => void;
    let release!: () => void;
    const entered = new Promise<void>((resolve) => { enter = resolve; });
    const held = new Promise<void>((resolve) => { release = resolve; });
    const fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
      const request = JSON.parse(String(init?.body));
      if (request.method === "initialize") return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: { protocolVersion: "2025-11-25" } }));
      if (request.method !== "tools/call") return new Response(null, { status: 202 });
      const name = request.params.name as string;
      calls.push(name);
      if (name === "transfer_begin") { enter(); await held; }
      const result = name === "transfer_begin" ? { transferId: "begun", size: 3, sha256: request.params.arguments.sha256, chunkBytes: 3 } : { ok: true };
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: { structuredContent: { result } } }));
    }) as typeof globalThis.fetch;
    const client = new McpClient("http://kernel.test/mcp", { fetch });
    const controller = new AbortController();
    const upload = client.upload(new Uint8Array([1, 2, 3]), "app_1", undefined, controller.signal);
    await entered;
    controller.abort();
    release();
    await expect(upload).rejects.toThrow();
    expect(calls).toEqual(["transfer_begin", "transfer_cancel"]);
  });

  test("cancels after a completed upload is refused and uses keepalive for cleanup", async () => {
    const calls: { name: string; keepalive?: boolean }[] = [];
    const fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
      const request = JSON.parse(String(init?.body));
      if (request.method === "initialize") return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: { protocolVersion: "2025-11-25" } }));
      if (request.method !== "tools/call") return new Response(null, { status: 202 });
      const name = request.params.name as string;
      calls.push({ name, keepalive: init?.keepalive });
      const result = name === "transfer_begin"
        ? { transferId: "begun", size: 3, sha256: request.params.arguments.sha256, chunkBytes: 3 }
        : { ok: true };
      const toolResult = name === "transfer_complete"
        ? { isError: true, content: [{ text: "Completion refused" }], structuredContent: { result: null } }
        : { structuredContent: { result } };
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: toolResult }));
    }) as typeof globalThis.fetch;
    const client = new McpClient("http://kernel.test/mcp", { fetch });
    await expect(client.upload(new Uint8Array([1, 2, 3]), "app_1")).rejects.toThrow("Completion refused");
    expect(calls.map(({ name }) => name)).toEqual(["transfer_begin", "transfer_write", "transfer_complete", "transfer_cancel"]);
    expect(calls.at(-1)?.keepalive).toBe(true);
  });

  test("cancels a download whose response has an invalid size", async () => {
    const { calls, client } = endpoint();
    await expect(client.download({ transferId: "bad", size: 32 * 1024 * 1024 + 1, sha256: "", chunkBytes: 1 })).rejects.toThrow("Invalid MCP download size");
    expect(calls.map((call) => call.name)).toEqual(["transfer_cancel"]);
  });
});

describe("McpClient.cancelSourceRead", () => {
  test("names the pending source request and keeps the cancellation alive", async () => {
    const calls: { name: string; arguments: Record<string, unknown>; keepalive?: boolean }[] = [];
    const fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
      const request = JSON.parse(String(init?.body));
      if (request.method === "initialize") return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: { protocolVersion: "2025-11-25" } }));
      if (request.method !== "tools/call") return new Response(null, { status: 202 });
      calls.push({ ...request.params, keepalive: init?.keepalive });
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: { structuredContent: { result: { cancelled: true } } } }));
    }) as typeof globalThis.fetch;
    const client = new McpClient("http://kernel.test/mcp", { fetch });
    await client.cancelSourceRead("app_1", "request-1");
    expect(calls).toEqual([{ name: "project_sources_read", arguments: { projectId: "app_1", cancelRequestId: "request-1" }, keepalive: true }]);
  });
});
