import { expect, test } from "bun:test";
import { checkFunctionCandidate } from "./function-candidate";
import type { ServerArtifact } from "../kernel/functions";

const artifact: ServerArtifact = {
  version: 1,
  abi: "konae-functions/1",
  manifest: { version: 1, functions: { orders: { entry: "functions/orders.ts", mount: "/api/orders", methods: ["GET"], access: "anonymous", actions: [] } } },
  bundles: { orders: "export const handle=()=>null;" },
};

test("candidate preparation reads back the completed scoped transfer and binds the reply", async () => {
  const calls: { name: string; args: Record<string, unknown> }[] = [];
  const call = async (name: string, args: Record<string, unknown>) => {
    calls.push({ name, args });
    if (name === "transfer_begin") return { transferId: "transfer", chunkBytes: 1_000_000 };
    if (name === "transfer_status") return { complete: true, size: calls[0].args.size, sha256: calls[0].args.sha256 };
    if (name === "function_candidate_check") return { status: "accepted", scratchRevision: args.scratchRevision, buildFingerprint: args.buildFingerprint };
    return {};
  };
  const result = await checkFunctionCandidate(call, "app_one", "revision-1", artifact);
  expect(result).toMatchObject({ status: "accepted", scratchRevision: "revision-1" });
  expect(calls.map(({ name }) => name)).toEqual(["transfer_begin", "transfer_write", "transfer_complete", "transfer_status", "function_candidate_check", "transfer_cancel"]);
  expect(calls[0].args.projectId).toBe("app_one");
  expect(calls[3].args.transferId).toBe("transfer");
});

test("candidate preparation discards a stale owner reply and cancels its transfer", async () => {
  const calls: { name: string; args: Record<string, unknown> }[] = [];
  const call = async (name: string, args: Record<string, unknown>) => {
    calls.push({ name, args });
    if (name === "transfer_begin") return { transferId: "transfer", chunkBytes: 1_000_000 };
    if (name === "transfer_status") return { complete: true, size: calls[0].args.size, sha256: calls[0].args.sha256 };
    if (name === "function_candidate_check") return { status: "accepted", scratchRevision: "older", buildFingerprint: args.buildFingerprint };
    return {};
  };
  await expect(checkFunctionCandidate(call, "app_one", "revision-2", artifact)).rejects.toThrow("stale or mismatched");
  expect(calls.at(-1)?.name).toBe("transfer_cancel");
});
