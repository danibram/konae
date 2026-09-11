import type { CapsuleManifest, RuntimeActionDeclaration } from "../deployments/manifest";
import { editorCapabilities } from "../deployments/manifest";

export type KernelGrant =
  | { type: "deployment"; deploymentId: string }
  | { type: "project-owner"; projectId: string };

export type KernelActor =
  | { kind: "anonymous"; actorId: null; grants: readonly [] }
  | { kind: "authenticated"; actorId: string; grants: readonly KernelGrant[] };

export type KernelDeployment = {
  id: string;
  projectId: string;
  manifest: CapsuleManifest;
};

export type KernelIntent =
  | { type: "open-deployment"; deployment: KernelDeployment }
  | { type: "invoke-action"; deployment: KernelDeployment; actionName: string }
  | { type: "admin"; projectId: string; capability: typeof editorCapabilities[number] };

export type KernelDecision =
  | { allowed: true; actorId: string | null; action?: RuntimeActionDeclaration }
  | {
    allowed: false;
    code: "authentication-required" | "deployment-grant-required" | "action-not-declared" | "action-grant-required" | "admin-forbidden";
  };

export const anonymousActor: KernelActor = { kind: "anonymous", actorId: null, grants: [] };

export function authorizeKernelIntent(actor: KernelActor, intent: KernelIntent): KernelDecision {
  if (intent.type === "admin") {
    return actor.kind === "authenticated" && hasProjectOwnerGrant(actor, intent.projectId)
      ? { allowed: true, actorId: actor.actorId }
      : { allowed: false, code: "admin-forbidden" };
  }

  const deploymentAccess = authorizeDeployment(actor, intent.deployment);
  if (!deploymentAccess.allowed) return deploymentAccess;
  if (intent.type === "open-deployment") return deploymentAccess;

  const action = intent.deployment.manifest.runtime.database.actions.find((candidate) => candidate.name === intent.actionName);
  if (!action) return { allowed: false, code: "action-not-declared" };
  if (action.access === "anonymous") return { allowed: true, actorId: actor.actorId, action };
  if (action.access === "authenticated") {
    return actor.kind === "authenticated"
      ? { allowed: true, actorId: actor.actorId, action }
      : { allowed: false, code: "authentication-required" };
  }
  return actor.kind === "authenticated" && hasDeploymentGrant(actor, intent.deployment)
    ? { allowed: true, actorId: actor.actorId, action }
    : { allowed: false, code: "action-grant-required" };
}

function authorizeDeployment(actor: KernelActor, deployment: KernelDeployment): KernelDecision {
  if (deployment.manifest.visibility === "public") return { allowed: true, actorId: actor.actorId };
  if (actor.kind === "anonymous") return { allowed: false, code: "authentication-required" };
  return hasDeploymentGrant(actor, deployment)
    ? { allowed: true, actorId: actor.actorId }
    : { allowed: false, code: "deployment-grant-required" };
}

function hasDeploymentGrant(actor: Extract<KernelActor, { kind: "authenticated" }>, deployment: KernelDeployment): boolean {
  return actor.grants.some((grant) =>
    (grant.type === "deployment" && grant.deploymentId === deployment.id)
    || (grant.type === "project-owner" && grant.projectId === deployment.projectId));
}

function hasProjectOwnerGrant(actor: Extract<KernelActor, { kind: "authenticated" }>, projectId: string): boolean {
  return actor.grants.some((grant) => grant.type === "project-owner" && grant.projectId === projectId);
}
