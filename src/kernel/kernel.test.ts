import { describe, expect, test } from "bun:test";
import { createDefaultManifest } from "../deployments/manifest";
import { KernelIdentity, MemoryKernelIdentityStore } from "./auth";
import { LittleboxKernel } from "./kernel";

describe("Littlebox Kernel", () => {
  test("authenticates and applies deployment visibility through one interface", async () => {
    const store = new MemoryKernelIdentityStore();
    store.setGrants("user_1", [{ type: "deployment", deploymentId: "deploy_1" }]);
    const identity = new KernelIdentity(store, { createToken: () => "opaque-session" });
    const kernel = new LittleboxKernel(identity);
    const session = await identity.issueSession({ actorId: "user_1" });
    const manifest = createDefaultManifest();
    manifest.visibility = "private";
    manifest.runtime.auth.mode = "required";
    const intent = {
      type: "open-deployment" as const,
      deployment: { id: "deploy_1", projectId: "project_1", manifest },
    };

    expect(await kernel.authorize(null, intent)).toEqual({ allowed: false, code: "authentication-required" });
    expect(await kernel.authorize("invalid-session", intent)).toEqual({ allowed: false, code: "authentication-required" });
    expect(await kernel.authorize(session.token, intent)).toEqual({ allowed: true, actorId: "user_1" });
  });
});
