import type { DeploymentReceipt } from "../deployments/client";
import { normalizeManifest } from "../deployments/manifest";
import { cloneStarter } from "./starter";
import type { MiniAppProject } from "./types";

export type DeploymentIndex = Record<string, DeploymentReceipt>;
export type WorkspaceState = { projects: MiniAppProject[]; activeProjectId: string; deployments: DeploymentIndex };

const activeProjectKey = "konae:active-project";
const deploymentsKey = "konae:deployments";

/** Browser storage keeps view state and the selected app ID, while app files load from the installation. */
export function loadWorkspaceState(storage: Pick<Storage, "getItem"> = localStorage): WorkspaceState {
  const deployments = readJson<DeploymentIndex>(storage.getItem(deploymentsKey)) ?? {};
  const legacyDeployment = readJson<DeploymentReceipt>(storage.getItem("konae:active-deployment"));
  if (legacyDeployment?.projectId && !deployments[legacyDeployment.projectId]) deployments[legacyDeployment.projectId] = withoutAccessCode(legacyDeployment);
  const project = cloneStarter();
  return { projects: [project], activeProjectId: storage.getItem(activeProjectKey) ?? project.id, deployments };
}

export function createWorkspaceProject(name: string, existing: readonly MiniAppProject[]): MiniAppProject {
  const project = cloneStarter();
  const normalizedName = name.trim().slice(0, 80) || "Untitled app";
  const baseSlug = slugify(normalizedName) || "untitled-app";
  const usedSlugs = new Set(existing.map((item) => item.slug));
  let slug = baseSlug;
  let suffix = 2;
  while (usedSlugs.has(slug)) slug = `${baseSlug}-${suffix++}`;
  return { ...project, id: `app_${crypto.randomUUID()}`, name: normalizedName, slug, updatedAt: Date.now() };
}

export function resetWorkspaceProject(project: MiniAppProject): MiniAppProject {
  const seed = cloneStarter();
  return { ...seed, id: project.id, name: project.name, slug: project.slug, updatedAt: Date.now() };
}

export function persistWorkspace(
  _projects: readonly MiniAppProject[],
  activeProjectId: string,
  deployments: DeploymentIndex,
  storage: Pick<Storage, "setItem"> = localStorage,
) {
  storage.setItem(activeProjectKey, activeProjectId);
  storage.setItem(deploymentsKey, JSON.stringify(Object.fromEntries(Object.entries(deployments).map(([projectId, deployment]) => [projectId, withoutAccessCode(deployment)]))));
}

function withoutAccessCode(deployment: DeploymentReceipt): DeploymentReceipt {
  return deployment;
}

function slugify(value: string) {
  return value.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 45);
}

function readJson<T>(value: string | null): T | null {
  if (!value) return null;
  try {
    return JSON.parse(value) as T;
  } catch {
    return null;
  }
}
