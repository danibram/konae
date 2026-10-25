import type { MiniAppRuntimeHost } from "../runtime/host";

const methods: Record<string, string> = {
  actions_invoke: "actions.invoke",
  auth_session: "auth.session",
  preview_sign_in: "auth.signIn",
  preview_sign_in_with_google: "auth.signInWithGoogle",
  auth_logout: "auth.signOut",
  files_list: "files.list",
  preview_files_upload: "files.upload",
  preview_files_url: "files.url",
  files_update: "files.update",
  files_delete: "files.delete",
  preview_ready: "runtime.ready",
  preview_error: "preview.error",
};

const fileTools = new Set(["files.upload", "files.update", "files.delete"]);

export type PreviewMcpRequest = {
  jsonrpc: "2.0";
  id?: string | number;
  method: string;
  params?: { name?: string; arguments?: unknown; _meta?: Record<string, unknown> };
};

export async function handlePreviewMcp(host: Pick<MiniAppRuntimeHost, "handle">, request: PreviewMcpRequest, capability: string) {
  const error = (code: number, message: string) => ({ jsonrpc: "2.0", id: request.id ?? null, error: { code, message } });
  if (request.jsonrpc !== "2.0" || request.params?._meta?.["littlebox/capability"] !== capability) return error(-32001, "Preview capability is invalid.");
  if (request.method.startsWith("notifications/")) return null;
  if (request.method === "initialize") return { jsonrpc: "2.0", id: request.id, result: { protocolVersion: "2025-11-25", capabilities: { tools: {} }, serverInfo: { name: "littlebox-preview", version: "0.1.0" } } };
  if (request.method === "tools/list") return { jsonrpc: "2.0", id: request.id, result: { tools: Object.keys(methods).map(name => ({ name, inputSchema: { type: "object" } })) } };
  if (request.method !== "tools/call") return error(-32601, "Unknown MCP method.");
  const method = request.params?.name && methods[request.params.name];
  if (!method) return error(-32602, "Unknown preview tool.");
  try {
    // Uploads, visibility changes and deletes require a signed-in preview
    // actor. Listing and files.url are allowed and the host returns only
    // public files plus that actor's private files.
    if (fileTools.has(method) && (await host.handle("auth.session", undefined) as { role?: string }).role === "anonymous") {
      throw new Error("authentication-required");
    }
    const result = await host.handle(method, request.params?.arguments);
    return { jsonrpc: "2.0", id: request.id, result: { content: [{ type: "text", text: JSON.stringify(result) }], structuredContent: { result }, isError: false } };
  } catch (cause) {
    return { jsonrpc: "2.0", id: request.id, result: { content: [{ type: "text", text: cause instanceof Error ? cause.message : String(cause) }], isError: true } };
  }
}
