import type { MiniAppProject } from "../lib/types";
import type { DeploymentRecord } from "./manifest";

type Released = Pick<DeploymentRecord, "createdAt" | "releasedAt">;
type Compared = Pick<DeploymentRecord, "manifest" | "actions" | "sourceDigest">;

export function releasedAt(deployment: Released): number {
  return deployment.releasedAt ?? deployment.createdAt;
}

export function ownerStorage(ownerId: string, storage: Storage = localStorage) {
  const key = (name: string) => `konae:owner:${encodeURIComponent(ownerId)}:${name}`;
  return {
    getItem: (name: string) => storage.getItem(key(name)),
    setItem: (name: string, value: string) => storage.setItem(key(name), value),
    removeItem: (name: string) => storage.removeItem(key(name)),
  };
}

export function accessNotDeployed(project: Pick<MiniAppProject, "files" | "manifest">, deployment: Compared | null | undefined): boolean {
  if (!deployment) return false;
  if (deployment.actions !== undefined) {
    const source = project.files["actions.json"];
    if (source === undefined) {
      try { if (Object.keys(JSON.parse(deployment.actions)?.actions ?? {}).length > 0) return true; } catch { return true; }
    } else if (source !== deployment.actions) return true;
  }
  if (project.manifest.visibility !== deployment.manifest.visibility) return true;
  return false;
}

export function sameAsDeployed(project: Pick<MiniAppProject, "files" | "manifest">, deployment: Compared | null | undefined, digest: string | null): boolean | null {
  if (!deployment) return null;
  if (accessNotDeployed(project, deployment)) return false;
  if (!deployment.sourceDigest || !digest) return null;
  return deployment.sourceDigest === digest;
}

export function changesNotDeployed(project: Pick<MiniAppProject, "files" | "manifest">, deployment: Compared | null | undefined, digest: string | null): boolean {
  return sameAsDeployed(project, deployment, digest) === false;
}
