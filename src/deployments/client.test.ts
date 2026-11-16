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
    let policy = { mode: "open", allowlist: [] as string[] };
    let publishedVisibility: "public" | "private" = "private";
    let draftVisibility: "public" | "private" = "public";
    globalThis.fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
      const request = JSON.parse(String(init?.body));
      if (request.method.startsWith("notifications/")) return new Response(null, { status: 202 });
      if (request.method === "initialize") return Response.json({ jsonrpc: "2.0", id: request.id, result: { protocolVersion: "2025-11-25" } });
      const { name, arguments: args } = request.params;
      calls.push({ name, args });
      if (request.params.name === "signup_policy_set") policy = { mode: args.mode, allowlist: args.allowlist };
      const value = request.params.name === "project_inspect"
        ? { project: { id: args.projectId, manifest: { visibility: draftVisibility } }, deployment: { id: args.projectId, manifest: { visibility: publishedVisibility }, signupPolicy: policy } }
        : policy;
      return Response.json({ jsonrpc: "2.0", id: request.id, result: { structuredContent: { result: value }, content: [], isError: false } });
    }) as typeof fetch;
    expect(await readSignupPolicy("deploy/a")).toMatchObject({ mode: "open", visibility: "private" });
    // A saved draft may have a different visibility from the published app.
    publishedVisibility = "public";
    draftVisibility = "private";
    expect(await readSignupPolicy("deploy/a")).toMatchObject({ mode: "open", visibility: "public" });
    expect(await saveSignupPolicy("deploy/a", "allowlist", ["@example.com"])).toMatchObject({ mode: "allowlist", allowlist: ["@example.com"] });
    expect(calls).toEqual([
      { name: "project_inspect", args: { projectId: "deploy/a" } },
      { name: "project_inspect", args: { projectId: "deploy/a" } },
      { name: "signup_policy_set", args: { projectId: "deploy/a", mode: "allowlist", allowlist: ["@example.com"] } },
      { name: "project_inspect", args: { projectId: "deploy/a" } },
    ]);
  });
});

const noChange = { statements: [], losses: [], confirmation: null };
const askNever = async () => { throw Error("asked about a change that loses nothing"); };

describe("a schema change that loses data", () => {
  const artifact = { version: 1 as const, entrypoint: "index.html", files: { "index.html": "<main>Test</main>" } };
  const dropping = { statements: ["DROP TABLE \"notes\""], losses: [{ table: "notes", column: null, sentence: "Drops table notes and its 2 rows." }], confirmation: "c".repeat(64) };

  test("is asked about, and its confirmation goes to release_apply", async () => {
    const calls = fakeKernel((name, args) => name === "transfer_begin" ? { transferId: "t", size: args.size, sha256: args.sha256, chunkBytes: 1 << 20 } : name === "release_plan" ? { planId: "p", schema: dropping } : {});
    const asked: unknown[] = [];
    await deployProject(starterProject, "public", artifact, async (change) => { asked.push(change); return true; });
    expect(asked).toEqual([dropping]);
    expect(calls.find((call) => call.name === "release_apply")?.args).toEqual({ planId: "p", confirmSchema: "c".repeat(64) });
  });

  test("declining a data transformation reports its rows without applying it", async () => {
    const change = { statements: [], losses: [], confirmation: "c".repeat(64), transformations: [{ id: "trim-notes", definitionSha256: "d".repeat(64), before: "before", after: "after", statements: ["UPDATE notes SET body=trim(body)"], maxRows: 10, affectedRows: 2, baselineSkipped: false }] };
    const calls = fakeKernel((name, args) => name === "transfer_begin" ? { transferId: "t", size: args.size, sha256: args.sha256, chunkBytes: 1 << 20 } : name === "release_plan" ? { planId: "p", schema: change } : {});
    await expect(deployProject(starterProject, "public", artifact, async () => false)).rejects.toThrow("Nothing was published. Transition trim-notes would change 2 rows, with a limit of 10.");
    expect(calls.map((call) => call.name)).not.toContain("release_apply");
    expect(calls.at(-1)?.name).toBe("transfer_cancel");
  });

  test("publishes nothing when the owner keeps the data", async () => {
    const calls = fakeKernel((name, args) => name === "transfer_begin" ? { transferId: "t", size: args.size, sha256: args.sha256, chunkBytes: 1 << 20 } : name === "release_plan" ? { planId: "p", schema: dropping } : {});
    await expect(deployProject(starterProject, "public", artifact, async () => false)).rejects.toThrow("Nothing was published. Drops table notes and its 2 rows.");
    expect(calls.map((call) => call.name)).not.toContain("release_apply");
    expect(calls.at(-1)?.name).toBe("transfer_cancel");
  });
});

describe("deployment transfer cleanup", () => {
  for (const failedTool of ["release_plan", "release_apply"]) {
    test(`cancels its upload after ${failedTool} fails`, async () => {
      const calls: string[] = [];
      const cancelled: string[] = [];
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
          if (name === "transfer_begin") value = { transferId: `publish-upload-${calls.filter((call) => call === "transfer_begin").length}`, size: args.size, sha256: args.sha256, chunkBytes: 65_536 };
          if (name === "release_plan") value = { planId: "publish-plan", schema: noChange };
          if (name === "transfer_cancel") cancelled.push(args.transferId);
          result = { structuredContent: { result: value }, content: [], isError: false };
        }
        return Response.json({ jsonrpc: "2.0", id: request.id, result });
      }) as typeof fetch;
      await expect(deployProject(starterProject, "public", { version: 1, entrypoint: "index.html", files: { "index.html": "<main>Test</main>" } }, askNever)).rejects.toThrow(`${failedTool} refused`);
      expect(calls.at(-1)).toBe("transfer_cancel");
      // Both uploads, the files of app/ and the release, are cancelled.
      expect(cancelled).toEqual(["publish-upload-1", "publish-upload-2"]);
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
      if (name === "release_plan") return { planId: "p", schema: noChange };
      return {};
    });
    await deployProject(starterProject, "private", artifact, askNever, initialSignupPolicy);
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
      { name: "app_users_list", args: { projectId: "deploy/a", limit: 25, cursor: "b@example.com", state: "suspended" } },
      { name: "app_invitations_create", args: { projectId: "deploy/a", emails: ["ana@example.com"], idempotencyKey: "invite-key-1" } },
      { name: "app_invitation_resend", args: { projectId: "deploy/a", invitationId: "inv_1", idempotencyKey: "resend-key-1" } },
    ]);
  });
});
