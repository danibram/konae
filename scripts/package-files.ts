// Streams package components between MCP transfers, files and the kit
// archive in fixed blocks, so a 256 MiB database never sits in memory.
import { createHash } from "node:crypto";
import { open, rm } from "node:fs/promises";
import { decodeBase64, McpError, type McpClient, type Transfer } from "../src/mcp/client";
import { normalizeArchivePath, tarHeader, tarPadding, type TarEntry } from "../src/lib/tar";

const block = 64 * 1024;

/** Reads a download transfer into `file`, checks size and SHA-256, and releases the transfer. */
export async function downloadToFile(client: McpClient, transfer: Transfer, file: string) {
  if (!Number.isSafeInteger(transfer.size) || transfer.size < 0) throw new McpError("Invalid MCP download size.");
  const hash = createHash("sha256");
  const output = await open(file, "wx", 0o600);
  try {
    let offset = 0;
    while (offset < transfer.size) {
      const read = await client.call<{ data: string; offset: number; nextOffset: number }>("transfer_read", { transferId: transfer.transferId, offset });
      const data = decodeBase64(read.data);
      if (read.offset !== offset || !data.length || data.length > block || read.nextOffset !== offset + data.length || read.nextOffset > transfer.size) {
        throw new McpError("Invalid MCP download block.");
      }
      await output.write(data, 0, data.length, offset);
      hash.update(data);
      offset = read.nextOffset;
    }
    await output.sync();
    if (hash.digest("hex") !== transfer.sha256) throw new McpError("MCP download checksum mismatch.");
  } finally {
    await output.close();
    await client.cancel(transfer.transferId).catch(() => {});
  }
}

export type FileTarEntry = { path: string; file: string; size: number; mode?: number };

/** Writes a ustar archive to `output`, copying file entries from disk in 1 MiB reads. Refuses to overwrite. */
export async function writeTarFile(output: string, entries: readonly (TarEntry | FileTarEntry)[]) {
  const target = await open(output, "wx", 0o600);
  const buffer = new Uint8Array(1024 * 1024);
  try {
    for (const entry of entries) {
      const path = normalizeArchivePath(entry.path);
      if ("file" in entry) {
        await target.write(tarHeader(path, entry.size, entry.mode ?? 0o600));
        const source = await open(entry.file, "r");
        try {
          let copied = 0;
          while (copied < entry.size) {
            const { bytesRead } = await source.read(buffer, 0, Math.min(buffer.length, entry.size - copied), copied);
            if (bytesRead === 0) throw new Error(`${entry.file} is shorter than its manifest size.`);
            await target.write(buffer.subarray(0, bytesRead));
            copied += bytesRead;
          }
        } finally {
          await source.close();
        }
        await target.write(new Uint8Array(tarPadding(entry.size)));
      } else {
        const contents = typeof entry.contents === "string" ? new TextEncoder().encode(entry.contents) : entry.contents;
        await target.write(tarHeader(path, contents.length, entry.mode ?? 0o644));
        await target.write(contents);
        await target.write(new Uint8Array(tarPadding(contents.length)));
      }
    }
    await target.write(new Uint8Array(1024));
    await target.sync();
  } catch (error) {
    await target.close();
    await rm(output, { force: true });
    throw error;
  }
  await target.close();
}
