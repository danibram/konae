import type { MiniAppProject } from "../lib/types";
import type { DeploymentRecord } from "./manifest";

type Released = Pick<DeploymentRecord, "createdAt" | "releasedAt">;
type Compared = Pick<DeploymentRecord, "manifest" | "actions" | "sourceDigest">;

/** When the release the app runs was published. A Kernel older than `releasedAt` names only the first publish. */
export function releasedAt(deployment: Released): number {
  return deployment.releasedAt ?? deployment.createdAt;
}

/** Keys under one owner, as the editor's workspace keeps them. */
export function ownerStorage(ownerId: string, storage: Storage = localStorage) {
  const key = (name: string) => `konae:owner:${encodeURIComponent(ownerId)}:${name}`;
  return {
    getItem: (name: string) => storage.getItem(key(name)),
    setItem: (name: string, value: string) => storage.setItem(key(name), value),
    removeItem: (name: string) => storage.removeItem(key(name)),
  };
}

/**
 * True when the release runs other access rules than the editor holds:
 * actions.json against the actions the release runs, which is what Export
 * reports as `sourcesAhead`, and who can open the app or read uploaded files
 * against the Deployment's manifest.
 */
export function accessNotDeployed(project: Pick<MiniAppProject, "files" | "manifest">, deployment: Compared | null | undefined): boolean {
  if (!deployment) return false;
  if (deployment.actions !== undefined && (project.files["actions.json"] ?? "") !== deployment.actions) return true;
  if (project.manifest.visibility !== deployment.manifest.visibility) return true;
  return (project.manifest.fileAccess ?? null) !== (deployment.manifest.fileAccess ?? null);
}

/**
 * Whether the published app runs what the editor holds. `digest` is
 * sourceDigest() of the editor's files, or null while it is computed.
 * - false: it runs other files or other access rules;
 * - true: same files and same access;
 * - null: unknown, because the release names no source digest (a Kernel or a
 *   record older than the field) or the editor's digest is not ready yet.
 */
export function sameAsDeployed(project: Pick<MiniAppProject, "files" | "manifest">, deployment: Compared | null | undefined, digest: string | null): boolean | null {
  if (!deployment) return null;
  if (accessNotDeployed(project, deployment)) return false;
  if (!deployment.sourceDigest || !digest) return null;
  return deployment.sourceDigest === digest;
}

/** True only when the editor holds something the published app does not run. An unknown answer says nothing. */
export function changesNotDeployed(project: Pick<MiniAppProject, "files" | "manifest">, deployment: Compared | null | undefined, digest: string | null): boolean {
  return sameAsDeployed(project, deployment, digest) === false;
}
