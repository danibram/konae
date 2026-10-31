import { expect, test } from "bun:test";
import { handlePreviewMcp } from "./preview";

test("preview MCP requires its private capability and only exposes runtime tools", async () => {
  const calls: string[] = [];
  const host = { handle: async (name: string) => { calls.push(name); return { userId: "alice" }; } };
  const request = { jsonrpc: "2.0" as const, id: "test", method: "tools/call", params: { name: "auth_session", _meta: { "konae/capability": "secret" } } };
  expect(await handlePreviewMcp(host, request, "different")).toHaveProperty("error.code", -32001);
  expect(calls).toHaveLength(0);
  expect(await handlePreviewMcp(host, request, "secret")).toHaveProperty("result.structuredContent.result.userId", "alice");
  expect(await handlePreviewMcp(host, { ...request, params: { ...request.params, name: "database_execute" } }, "secret")).toHaveProperty("error.code", -32602);
  expect(calls).toEqual(["auth.session"]);
});

test("preview MCP lets a signed-out visitor list files and refuses uploads", async () => {
  let role = "anonymous";
  const calls: string[] = [];
  const host = { handle: async (name: string) => { calls.push(name); return name === "auth.session" ? { role } : []; } };
  const call = (name: string) => handlePreviewMcp(host, { jsonrpc: "2.0", id: "f", method: "tools/call", params: { name, _meta: { "konae/capability": "secret" } } }, "secret");
  expect(await call("files_list")).toMatchObject({ result: { isError: false } });
  expect(await call("preview_files_upload")).toMatchObject({ result: { isError: true, content: [{ text: "authentication-required" }] } });
  expect(calls).toEqual(["files.list", "auth.session"]);
  role = "member";
  expect(await call("preview_files_upload")).toMatchObject({ result: { isError: false } });
  expect(calls).toContain("files.upload");
});

test("preview MCP refuses a signed-out visitor's visibility change and delete, and lets files.url through", async () => {
  let role = "anonymous";
  const calls: string[] = [];
  const host = { handle: async (name: string) => { calls.push(name); return name === "auth.session" ? { role } : "data:,"; } };
  const call = (name: string) => handlePreviewMcp(host, { jsonrpc: "2.0", id: "f", method: "tools/call", params: { name, _meta: { "konae/capability": "secret" } } }, "secret");
  for (const tool of ["files_update", "files_delete"]) {
    expect(await call(tool)).toMatchObject({ result: { isError: true, content: [{ text: "authentication-required" }] } });
  }
  expect(await call("preview_files_url")).toMatchObject({ result: { isError: false, structuredContent: { result: "data:," } } });
  expect(calls).toEqual(["auth.session", "auth.session", "files.url"]);
  role = "member";
  expect(await call("files_delete")).toMatchObject({ result: { isError: false } });
  expect(calls).toContain("files.delete");
});
