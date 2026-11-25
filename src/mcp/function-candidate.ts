import { checksum } from "./client";
import type { ServerArtifact } from "../kernel/functions";

export type CandidateCall = (name: string, args: Record<string, unknown>, signal?: AbortSignal) => Promise<unknown>;

export async function checkFunctionCandidate(
  call: CandidateCall,
  projectId: string,
  scratchRevision: string,
  artifact: ServerArtifact,
  signal?: AbortSignal,
): Promise<{ status: "accepted"; scratchRevision: string; buildFingerprint: string }> {
  const bytes = new TextEncoder().encode(JSON.stringify(artifact));
  const buildFingerprint = await checksum(bytes);
  const upload = await call("transfer_begin", { projectId, size: bytes.length, sha256: buildFingerprint }) as { transferId: string; chunkBytes: number };
  try {
    if (!Number.isSafeInteger(upload.chunkBytes) || upload.chunkBytes < 1) throw new Error("The owner MCP returned an invalid transfer block size.");
    for (let offset = 0; offset < bytes.length; offset += upload.chunkBytes) {
      signal?.throwIfAborted();
      const part = bytes.subarray(offset, offset + upload.chunkBytes);
      let binary = "";
      for (let index = 0; index < part.length; index += 0x8000) binary += String.fromCharCode(...part.subarray(index, index + 0x8000));
      await call("transfer_write", { transferId: upload.transferId, offset, data: btoa(binary) }, signal);
    }
    await call("transfer_complete", { transferId: upload.transferId }, signal);
    const completed = await call("transfer_status", { transferId: upload.transferId }, signal) as { complete?: boolean; size?: number; sha256?: string };
    if (completed.complete !== true || completed.size !== bytes.length || completed.sha256 !== buildFingerprint) {
      throw new Error("The owner MCP transfer read-back does not match the candidate upload.");
    }
    const result = await call("function_candidate_check", {
      projectId, transferId: upload.transferId, scratchRevision, buildFingerprint,
    }, signal) as { status: string; scratchRevision: string; buildFingerprint: string };
    if (result.status !== "accepted" || result.scratchRevision !== scratchRevision || result.buildFingerprint !== buildFingerprint) {
      throw new Error("The owner MCP returned a stale or mismatched function preparation result.");
    }
    return { status: "accepted", scratchRevision, buildFingerprint };
  } finally {
    await call("transfer_cancel", { transferId: upload.transferId }).catch(() => {});
  }
}
