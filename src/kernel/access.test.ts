import { describe, expect, test } from "bun:test";
import { createDefaultManifest } from "../deployments/manifest";
import { anonymousActor, authorizeKernelIntent, type KernelActor, type KernelDeployment } from "./access";

const member: KernelActor = {
  kind: "authenticated",
  actorId: "user_1",
  grants: [{ type: "deployment", deploymentId: "deploy_1" }],
};
const owner: KernelActor = {
  kind: "authenticated",
  actorId: "owner_1",
  grants: [{ type: "project-owner", projectId: "project_1" }],
};

function deployment(visibility: "public" | "private"): KernelDeployment {
  const manifest = createDefaultManifest();
  manifest.visibility = visibility;
  manifest.runtime.auth.mode = visibility === "private" ? "required" : "optional";
  return { id: "deploy_1", projectId: "project_1", manifest };
}

describe("Littlebox Kernel access", () => {
  test("serves public shells without exposing undeclared actions", () => {
    const publicDeployment = deployment("public");
    expect(authorizeKernelIntent(anonymousActor, { type: "open-deployment", deployment: publicDeployment }).allowed).toBe(true);
    expect(authorizeKernelIntent(anonymousActor, { type: "invoke-action", deployment: publicDeployment, actionName: "database.export" })).toEqual({
      allowed: false,
      code: "action-not-declared",
    });
  });

  test("gates private shells using Littlebox deployment grants", () => {
    const privateDeployment = deployment("private");
    expect(authorizeKernelIntent(anonymousActor, { type: "open-deployment", deployment: privateDeployment })).toEqual({
      allowed: false,
      code: "authentication-required",
    });
    expect(authorizeKernelIntent({ kind: "authenticated", actorId: "outsider", grants: [] }, { type: "open-deployment", deployment: privateDeployment })).toEqual({
      allowed: false,
      code: "deployment-grant-required",
    });
    expect(authorizeKernelIntent(member, { type: "open-deployment", deployment: privateDeployment }).allowed).toBe(true);
  });

  test("keeps project administration on the owner interface", () => {
    expect(authorizeKernelIntent(member, { type: "admin", projectId: "project_1", capability: "database:query" })).toEqual({
      allowed: false,
      code: "admin-forbidden",
    });
    expect(authorizeKernelIntent(owner, { type: "admin", projectId: "project_1", capability: "database:export" }).allowed).toBe(true);
  });

  test("applies action access after deployment visibility", () => {
    const publicDeployment = deployment("public");
    publicDeployment.manifest.runtime.database.actions.push({ name: "notes.private", kind: "query", access: "authenticated" });
    expect(authorizeKernelIntent(anonymousActor, { type: "invoke-action", deployment: publicDeployment, actionName: "notes.private" })).toEqual({
      allowed: false,
      code: "authentication-required",
    });
    expect(authorizeKernelIntent(member, { type: "invoke-action", deployment: publicDeployment, actionName: "notes.private" }).allowed).toBe(true);
  });
});
