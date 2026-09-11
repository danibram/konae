import type { DeploymentReceipt } from "../deployments/client";
import { normalizeManifest } from "../deployments/manifest";
import { architectureProjectId, cloneArchitectureProject } from "./architecture";
import { cloneStarter, starterDatabaseModelSource } from "./starter";
import type { MiniAppProject } from "./types";
import { directoriesForFiles } from "./project-files";

export type DeploymentIndex = Record<string, DeploymentReceipt>;

export type WorkspaceState = {
  projects: MiniAppProject[];
  activeProjectId: string;
  deployments: DeploymentIndex;
};

const projectsKey = "littlebox:projects";
const activeProjectKey = "littlebox:active-project";
const deploymentsKey = "littlebox:deployments";

export function loadWorkspaceState(storage: Pick<Storage, "getItem"> = localStorage): WorkspaceState {
  // One-way migration: the pre-workspace single-project key is read for owners
  // whose last save predates `littlebox:projects`, and is never written again.
  const legacyProject = readJson<MiniAppProject>(storage.getItem("littlebox:project"));
  const storedProjects = readJson<MiniAppProject[]>(storage.getItem(projectsKey));
  const projects = includeSeedProjects(normalizeProjects(Array.isArray(storedProjects) && storedProjects.length > 0
    ? storedProjects
    : legacyProject ? [legacyProject] : [cloneStarter()]));
  const requestedProjectId = storage.getItem(activeProjectKey);
  const activeProjectId = projects.some((project) => project.id === requestedProjectId)
    ? requestedProjectId as string
    : projects[0].id;

  const deployments = readJson<DeploymentIndex>(storage.getItem(deploymentsKey)) ?? {};
  const legacyDeployment = readJson<DeploymentReceipt>(storage.getItem("littlebox:active-deployment"));
  if (legacyDeployment?.projectId && !deployments[legacyDeployment.projectId]) {
    deployments[legacyDeployment.projectId] = withoutAccessCode(legacyDeployment);
  }

  return { projects, activeProjectId, deployments };
}

export function createWorkspaceProject(name: string, existing: readonly MiniAppProject[]): MiniAppProject {
  const project = cloneStarter();
  const normalizedName = name.trim().slice(0, 80) || "Untitled app";
  const baseSlug = slugify(normalizedName) || "untitled-app";
  const usedSlugs = new Set(existing.map((item) => item.slug));
  let slug = baseSlug;
  let suffix = 2;
  while (usedSlugs.has(slug)) slug = `${baseSlug}-${suffix++}`;
  return {
    ...project,
    id: `app_${crypto.randomUUID()}`,
    name: normalizedName,
    slug,
    updatedAt: Date.now(),
  };
}

export function resetWorkspaceProject(project: MiniAppProject): MiniAppProject {
  const seed = project.id === architectureProjectId ? cloneArchitectureProject() : cloneStarter();
  return {
    ...seed,
    id: project.id,
    name: project.name,
    slug: project.slug,
    updatedAt: Date.now(),
  };
}

export function persistWorkspace(
  projects: readonly MiniAppProject[],
  activeProjectId: string,
  deployments: DeploymentIndex,
  storage: Pick<Storage, "setItem"> = localStorage,
) {
  storage.setItem(projectsKey, JSON.stringify(projects));
  storage.setItem(activeProjectKey, activeProjectId);
  storage.setItem(deploymentsKey, JSON.stringify(
    Object.fromEntries(Object.entries(deployments).map(([projectId, deployment]) => [projectId, withoutAccessCode(deployment)])),
  ));
}

function normalizeProjects(projects: MiniAppProject[]) {
  const starter = cloneStarter();
  const seen = new Set<string>();
  return projects.flatMap((project) => {
    if (!project?.id || seen.has(project.id)) return [];
    seen.add(project.id);
    return [{
      ...project,
      files: { ...starter.files, ...project.files },
      directories: project.directories ?? directoriesForFiles(project.files ?? {}),
      databaseModel: project.databaseModel ?? legacyDatabaseModel(project.files?.["schema.sql"] ?? ""),
      manifest: normalizeManifest(project.manifest),
    }];
  });
}

function legacyDatabaseModel(schema: string) {
  const source = /CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?tasks\b/i.test(schema)
    ? starterDatabaseModelSource
    : `import { sqliteTable } from "drizzle-orm/sqlite-core";\n\n// Legacy schema.sql requires conversion before its first managed migration.\nvoid sqliteTable;\n`;
  return { source, acceptedSource: source, revision: "legacy-baseline" };
}

function includeSeedProjects(projects: MiniAppProject[]) {
  return projects.some((project) => project.id === architectureProjectId)
    ? projects
    : [...projects, cloneArchitectureProject()];
}

function withoutAccessCode(deployment: DeploymentReceipt): DeploymentReceipt {
  const { accessCode: _accessCode, ...safe } = deployment;
  return safe;
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
