export type Transfer = { transferId: string; size: number; sha256: string; chunkBytes: number };

/** A Capsule refusal a tool answers beside its text (ADR-0020). */
export type McpRefusal = { code: string; message: string; hint?: { fix: string; remedy: string; tool?: string } };

export class McpError extends Error {
  constructor(message: string, readonly status?: number, readonly refusal?: McpRefusal) { super(message); }
}

export class McpClient {
  private connecting?: Promise<void>;

  constructor(
    readonly endpoint = "/mcp",
    private readonly options: { token?: string; ownerId?: string; fetch?: typeof fetch } = {},
  ) {}

  async connect() {
    if (!this.connecting) {
      this.connecting = (async () => {
        const result = await this.request("initialize", {
          protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "konae", version: "0.1.0" },
        });
        if (result.protocolVersion !== "2025-11-25") throw new McpError("Unsupported MCP protocol version.");
        await this.request("notifications/initialized", undefined, true);
      })().catch((error) => { this.connecting = undefined; throw error; });
    }
    return this.connecting;
  }

  async call<T = unknown>(name: string, args: object = {}, signal?: AbortSignal): Promise<T> {
    await this.connect();
    const result = await this.request("tools/call", { name, arguments: args }, false, signal);
    if (result.isError) throw new McpError(result.content?.[0]?.text ?? "MCP tool failed.", undefined, result.structuredContent?.error);
    if (!result.structuredContent || !("result" in result.structuredContent)) throw new McpError("The MCP tool returned no structured result.");
    return result.structuredContent.result as T;
  }

  /** The tools this credential may call, as the Kernel lists them for it. */
  async listTools(signal?: AbortSignal): Promise<{ name: string; description?: string; inputSchema?: Record<string, unknown> }[]> {
    await this.connect();
    const result = await this.request("tools/list", {}, false, signal);
    return Array.isArray(result?.tools) ? result.tools : [];
  }

  /**
   * Uploads bytes as one owner transfer. `component` names a Package
   * component (`database`, `identity`, `files` or `usage`), which lets the
   * upload reach that component's limit instead of 32 MiB; a release then
   * names the transfer in `initialPackage.components`.
   */
  async upload(bytes: Uint8Array, projectId?: string, component?: string): Promise<Transfer> {
    const sha256 = await checksum(bytes);
    const transfer = await this.call<Transfer>("transfer_begin", { size: bytes.length, sha256, ...(projectId ? { projectId } : {}), ...(component ? { component } : {}) });
    try {
      for (let offset = 0; offset < bytes.length; offset += transfer.chunkBytes) {
        await this.call("transfer_write", { transferId: transfer.transferId, offset, data: encodeBase64(bytes.subarray(offset, offset + transfer.chunkBytes)) });
      }
      await this.call("transfer_complete", { transferId: transfer.transferId });
      return transfer;
    } catch (error) {
      await this.cancel(transfer.transferId).catch(() => {});
      throw error;
    }
  }

  async download(transfer: Transfer): Promise<Uint8Array> {
    if (!Number.isSafeInteger(transfer.size) || transfer.size < 0 || transfer.size > 32 * 1024 * 1024) throw new McpError("Invalid MCP download size.");
    const bytes = new Uint8Array(transfer.size);
    try {
      let offset = 0;
      while (offset < transfer.size) {
        const block = await this.call<{ data: string; offset: number; nextOffset: number }>("transfer_read", { transferId: transfer.transferId, offset });
        const data = decodeBase64(block.data);
        if (block.offset !== offset || !data.length || data.length > 64 * 1024 || block.nextOffset !== offset + data.length || block.nextOffset > bytes.length) throw new McpError("Invalid MCP download block.");
        bytes.set(data, offset);
        offset = block.nextOffset;
      }
      if (await checksum(bytes) !== transfer.sha256) throw new McpError("MCP download checksum mismatch.");
      return bytes;
    } finally {
      await this.cancel(transfer.transferId).catch(() => {});
    }
  }

  cancel(transferId: string) { return this.call("transfer_cancel", { transferId }); }

  private async request(method: string, params?: object, notification = false, signal?: AbortSignal): Promise<any> {
    const id = crypto.randomUUID();
    const response = await (this.options.fetch ?? globalThis.fetch)(this.endpoint, {
      method: "POST", credentials: "same-origin", signal,
      headers: {
        "content-type": "application/json", accept: "application/json, text/event-stream", "MCP-Protocol-Version": "2025-11-25",
        ...(this.options.token ? { authorization: `Bearer ${this.options.token}` } : {}),
        ...(this.options.ownerId ? { "x-konae-owner-id": this.options.ownerId } : {}),
      },
      body: JSON.stringify({ jsonrpc: "2.0", ...(!notification ? { id } : {}), method, ...(params ? { params } : {}) }),
    });
    if (notification && response.status === 202) return;
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      if (response.status === 401) this.connecting = undefined;
      throw new McpError(typeof body.error === "string" ? body.error : body.error?.message ?? "MCP request failed.", response.status);
    }
    if (body.jsonrpc !== "2.0" || body.id !== id) throw new McpError("Invalid MCP response ID.");
    if (body.error) throw new McpError(body.error.message ?? "MCP protocol error.");
    return body.result;
  }
}

export const platformMcp = new McpClient();

export async function checksum(bytes: Uint8Array): Promise<string> {
  const hash = await crypto.subtle.digest("SHA-256", new Uint8Array(bytes).buffer);
  return Array.from(new Uint8Array(hash), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function encodeBase64(bytes: Uint8Array): string {
  let text = "";
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text);
}

export function decodeBase64(value: string): Uint8Array {
  return Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
}
