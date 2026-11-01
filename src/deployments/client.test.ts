import { afterEach, describe, expect, test } from "bun:test";
import { HostedDatabaseClient, createAppInvitations, deployProject, listAppUsers, readSignupPolicy, resendAppInvitation, saveSignupPolicy } from "./client";
import { checksum, decodeBase64, encodeBase64 } from "../mcp/client";
import { starterProject } from "../lib/starter";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
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

/** Answers every tool with `answer(name, args)` and records the calls. */
function fakeKernel(answer: (name: string, args: Record<string, unknown>) => unknown = () => ({})) {
  const calls: { name: string; args: Record<string, unknown> }[] = [];
  globalThis.fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
    const request = JSON.parse(String(init?.body));
    if (request.method.startsWith("notifications/")) return new Response(null, { status: 202 });
    if (request.method === "initialize") return Response.json({ jsonrpc: "2.0", id: request.id, result: { protocolVersion: "2025-11-25" } });
    const { name, arguments: args } = request.params;
    calls.push({ name, args });
    return Response.json({ jsonrpc: "2.0", id: request.id, result: { structuredContent: { result: answer(name, args) }, content: [], isError: false } });
  }) as typeof fetch;
  return calls;
}

describe("first publish signup policy", () => {
  const artifact = { version: 1 as const, entrypoint: "index.html", files: { "index.html": "<main>Test</main>" } };

  async function uploaded(initialSignupPolicy?: { mode: "invitation"; allowlist: string[] }) {
    let payload: Record<string, unknown> = {};
    fakeKernel((name, args) => {
      if (name === "transfer_begin") return { transferId: "t", size: args.size, sha256: args.sha256, chunkBytes: 1 << 20 };
      if (name === "transfer_write") payload = JSON.parse(new TextDecoder().decode(decodeBase64(String(args.data))));
      if (name === "release_plan") return { planId: "p" };
      return {};
    });
    await deployProject(starterProject, "private", artifact, initialSignupPolicy);
    return payload;
  }

  test("sends the chosen policy next to the visibility", async () => {
    expect(await uploaded({ mode: "invitation", allowlist: [] })).toMatchObject({ visibility: "private", initialSignupPolicy: { mode: "invitation", allowlist: [] } });
  });

  test("sends no policy when none is given, as a republish must", async () => {
    expect(await uploaded()).not.toHaveProperty("initialSignupPolicy");
  });
});

describe("app users and invitations", () => {
  test("passes the deployment, the page and the idempotency key to the Kernel tools", async () => {
    const calls = fakeKernel((name) => name === "app_users_list" ? { users: [], nextCursor: null } : { results: [] });
    await listAppUsers("deploy/a", { limit: 25, cursor: "b@example.com", state: "suspended" });
    await createAppInvitations("deploy/a", ["ana@example.com"], "invite-key-1");
    await resendAppInvitation("deploy/a", "inv_1", "resend-key-1");
    expect(calls).toEqual([
      { name: "app_users_list", args: { deploymentId: "deploy/a", limit: 25, cursor: "b@example.com", state: "suspended" } },
      { name: "app_invitations_create", args: { deploymentId: "deploy/a", emails: ["ana@example.com"], idempotencyKey: "invite-key-1" } },
      { name: "app_invitation_resend", args: { deploymentId: "deploy/a", invitationId: "inv_1", idempotencyKey: "resend-key-1" } },
    ]);
  });
});
