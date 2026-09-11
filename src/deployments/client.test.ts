import { afterEach, describe, expect, test } from "bun:test";
import { HostedDatabaseClient, deployProject, readSignupPolicy, saveSignupPolicy } from "./client";
import { checksum, encodeBase64 } from "../mcp/client";
import { starterProject } from "../lib/starter";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe("HostedDatabaseClient portable export", () => {
  test("exports portable state through authenticated MCP and verifies its checksum", async () => {
    const bytes = new TextEncoder().encode(JSON.stringify({ result: { initialDatabase: "AAH/", initialIdentity: "AQID", initialFiles: "BAUG", initialSignupPolicy: { mode: "closed", allowlist: [] }, visibility: "private" } }));
    const sha256 = await checksum(bytes);
    const names: string[] = [];
    globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(String(input)).toBe("/mcp");
      expect(init?.credentials).toBe("same-origin");
      const request = JSON.parse(String(init?.body));
      if (request.method.startsWith("notifications/")) return new Response(null, { status: 202 });
      let result: unknown;
      if (request.method === "initialize") result = { protocolVersion: "2025-11-25" };
      else {
        const { name, arguments: args } = request.params;
        names.push(name);
        let value: unknown = {};
        if (name === "project_export") { expect(args.deploymentId).toBe("deploy/a"); value = { transferId: "export", size: bytes.length, sha256 }; }
        if (name === "transfer_read") value = { offset: 0, nextOffset: bytes.length, data: encodeBase64(bytes) };
        result = { structuredContent: { result: value }, content: [], isError: false };
      }
      return Response.json({ jsonrpc: "2.0", id: request.id, result });
    }) as typeof fetch;
    const state = await new HostedDatabaseClient("deploy/a").exportPortableState();
    expect(names).toEqual(["project_export", "transfer_read", "transfer_cancel"]);
    expect(state.initialDatabase).toEqual(Uint8Array.from([0, 1, 255]));
    expect(state.initialIdentity).toEqual(Uint8Array.from([1, 2, 3]));
    expect(state.initialFiles).toEqual(Uint8Array.from([4, 5, 6]));
    expect(state.initialSignupPolicy).toEqual({ mode: "closed", allowlist: [] });
    expect(state.visibility).toBe("private");
  });

  test("surfaces an MCP export error without returning partial state", async () => {
    globalThis.fetch = (async (_input, init) => {
      const request = JSON.parse(String(init?.body));
      return Response.json({ jsonrpc: "2.0", id: request.id, result: { isError: true, content: [{ type: "text", text: "Export unavailable." }] } });
    }) as typeof fetch;
    await expect(new HostedDatabaseClient("deploy_1").exportPortableState()).rejects.toThrow("Export unavailable.");
  });
});

describe("signup policy", () => {
  test("reads and saves the policy through the owner MCP tools", async () => {
    const calls: { name: string; args: Record<string, unknown> }[] = [];
    globalThis.fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
      const request = JSON.parse(String(init?.body));
      if (request.method.startsWith("notifications/")) return new Response(null, { status: 202 });
      if (request.method === "initialize") return Response.json({ jsonrpc: "2.0", id: request.id, result: { protocolVersion: "2025-11-25" } });
      const { name, arguments: args } = request.params;
      calls.push({ name, args });
      const value = { deploymentId: args.deploymentId, visibility: "public", mode: args.mode ?? "open", allowlist: args.allowlist ?? [] };
      return Response.json({ jsonrpc: "2.0", id: request.id, result: { structuredContent: { result: value }, content: [], isError: false } });
    }) as typeof fetch;
    expect(await readSignupPolicy("deploy/a")).toMatchObject({ mode: "open" });
    expect(await saveSignupPolicy("deploy/a", "allowlist", ["@example.com"])).toMatchObject({ mode: "allowlist", allowlist: ["@example.com"] });
    expect(calls).toEqual([
      { name: "signup_policy_read", args: { deploymentId: "deploy/a" } },
      { name: "signup_policy_set", args: { deploymentId: "deploy/a", mode: "allowlist", allowlist: ["@example.com"] } },
    ]);
  });
});

describe("deployment transfer cleanup", () => {
  for (const failedTool of ["release_plan", "release_apply"]) {
    test(`cancels its upload after ${failedTool} fails`, async () => {
      const calls: string[] = [];
      globalThis.fetch = (async (_input, init) => {
        const request = JSON.parse(String(init?.body));
        if (request.method.startsWith("notifications/")) return new Response(null, { status: 202 });
        if (request.method === "initialize") return Response.json({ jsonrpc: "2.0", id: request.id, result: { protocolVersion: "2025-11-25" } });
        const { name, arguments: args } = request.params;
        calls.push(name);
        let result: unknown;
        if (name === failedTool) result = { isError: true, content: [{ type: "text", text: `${name} refused` }] };
        else {
          let value: unknown = {};
          if (name === "transfer_begin") value = { transferId: "publish-upload", size: args.size, sha256: args.sha256, chunkBytes: 65_536 };
          if (name === "release_plan") value = { planId: "publish-plan" };
          if (name === "transfer_cancel") expect(args.transferId).toBe("publish-upload");
          result = { structuredContent: { result: value }, content: [], isError: false };
        }
        return Response.json({ jsonrpc: "2.0", id: request.id, result });
      }) as typeof fetch;
      await expect(deployProject(starterProject, "public", { version: 1, entrypoint: "index.html", files: { "index.html": "<main>Test</main>" } })).rejects.toThrow(`${failedTool} refused`);
      expect(calls.at(-1)).toBe("transfer_cancel");
      expect(calls.filter((name) => name === "transfer_cancel")).toHaveLength(1);
      if (failedTool === "release_plan") expect(calls).not.toContain("release_apply");
    });
  }
});
