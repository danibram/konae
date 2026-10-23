import type { DeploymentReceipt } from "../deployments/client";
import { normalizeManifest } from "../deployments/manifest";
import { architectureProjectId, cloneArchitectureProject } from "./architecture";
import { cloneStarter, repairDroppedPreviewUpload, starterDatabaseModelSource } from "./starter";
import type { MiniAppProject } from "./types";
import { directoriesForFiles } from "./project-files";

export type DeploymentIndex = Record<string, DeploymentReceipt>;

export type WorkspaceState = {
  projects: MiniAppProject[];
  activeProjectId: string;
  deployments: DeploymentIndex;
};

/** Keep the newest local draft per project and omit copies of the saved sources and of the seeded starter projects. */
export function recoveryDrafts(candidates: MiniAppProject[], saved: MiniAppProject[]): MiniAppProject[] {
  const latest = new Map<string, MiniAppProject>();
  for (const candidate of candidates) {
    if (!candidate || typeof candidate.id !== "string" || !candidate.files || typeof candidate.files !== "object") continue;
    const previous = latest.get(candidate.id);
    if (!previous || candidate.updatedAt > previous.updatedAt) latest.set(candidate.id, candidate);
  }
  const current = new Map(saved.map((project) => [project.id, draftSignature(project)]));
  // The browser used to seed the starter and the architecture map into every empty store. The map is no longer a project the editor opens, so no copy of it is a draft; an unedited starter is not something the owner made.
  const starter = cloneStarter();
  const starterSignature = draftSignature(starter);
  return [...latest.values()].filter((draft) => current.get(draft.id) !== draftSignature(draft) && draft.id !== architectureProjectId && !(draft.id === starter.id && draftSignature(draft) === starterSignature));
}

function draftSignature(project: MiniAppProject): string {
  const repaired = repairDroppedPreviewUpload(project);
  const canonical = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(canonical);
    if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, item]) => [key, canonical(item)]));
    return value;
  };
  return JSON.stringify(canonical({ name: repaired.name, slug: repaired.slug, files: repaired.files, directories: [...(repaired.directories ?? [])].sort(), model: repaired.databaseModel?.source, manifest: repaired.manifest }));
}

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

/**
 * The projects a browser actually stored, for the "Import apps from this
 * browser" list. loadWorkspaceState invents a starter and the architecture map
 * for an empty store, and offering those would list apps the owner never made.
 */
export function storedLocalProjects(storage: Pick<Storage, "getItem"> = localStorage): MiniAppProject[] {
  const legacyProject = readJson<MiniAppProject>(storage.getItem("littlebox:project"));
  const storedProjects = readJson<MiniAppProject[]>(storage.getItem(projectsKey));
  const stored = Array.isArray(storedProjects) && storedProjects.length > 0 ? storedProjects : legacyProject ? [legacyProject] : [];
  return normalizeProjects(stored).filter((project) => project.id !== architectureProjectId);
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
    const refreshed = refreshUneditedSeed({
      ...project,
      files: { ...starter.files, ...project.files },
    });
    return [repairDroppedPreviewUpload({
      ...refreshed,
      directories: project.directories ?? directoriesForFiles(refreshed.files ?? {}),
      databaseModel: project.databaseModel ?? legacyDatabaseModel(project.files?.["schema.sql"] ?? ""),
      manifest: normalizeManifest(project.manifest),
    })];
  });
}

function legacyDatabaseModel(schema: string) {
  const source = /CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?tasks\b/i.test(schema)
    ? starterDatabaseModelSource
    : `import { sqliteTable } from "drizzle-orm/sqlite-core";\n\n// Legacy schema.sql requires conversion before its first managed migration.\nvoid sqliteTable;\n`;
  return { source, acceptedSource: source, revision: "legacy-baseline" };
}

function refreshUneditedSeed(project: MiniAppProject): MiniAppProject {
  const architectureHtml = project.files["index.html"] ?? "";
  const architectureStyle = project.files["styles.css"] ?? "";
  const architectureScript = project.files["app.js"] ?? "";
  if (project.id === architectureProjectId && (architectureHtml.includes("mapa del sistema") || architectureHtml.includes("REGLA BASE") || architectureHtml.includes("KERNEL ONLINE") || architectureHtml.includes("kernel that decides") || architectureHtml.includes("Dos caminos") || architectureHtml.includes("declared-actions") || architectureHtml.includes("De código") || architectureHtml.includes("Sólo los campos") || architectureHtml.includes("runtime no disponible") || architectureHtml.includes("frame-ancestors") || !architectureHtml.includes('data-map="leaves"') || architectureStyle.includes("opacity: 0") || architectureScript.includes("localhost:8080") || architectureHtml.includes("database stays on the server") || architectureHtml.includes("SIMULADOR") || architectureHtml.includes("Keep SQLite") || architectureHtml.includes("SQLite and files") || !architectureHtml.includes('data-lead="same-app"') || !architectureHtml.includes('data-lead="edit-files"'))) {
    const seed = cloneArchitectureProject().files;
    return { ...project, files: { ...project.files, "index.html": seed["index.html"], "styles.css": seed["styles.css"], "app.js": seed["app.js"] } };
  }
  const html = project.files["index.html"] ?? "";
  const css = project.files["styles.css"] ?? "";
  const script = project.files["app.js"] ?? "";
  const previousStarter = project.id === "app_tiny_tasks"
    && html.includes('class="identity">guest')
    && !css.includes("prefers-color-scheme")
    && script.includes("identity.textContent = session.name;");
  if (!previousStarter && project.id === "app_tiny_tasks") {
    let nextHtml = html;
    let nextScript = script;
    let changed = false;
    if (nextHtml.includes("Things worth") || nextHtml.includes("MY QUIET LIST") || nextHtml.includes("My quiet list")) {
      nextHtml = nextHtml
        .replace(/<p class="eyebrow">[^<]*<\/p>/, '<p class="eyebrow">Shared list</p>')
        .replace(/<h1>[\s\S]*?<\/h1>/, "<h1>Tiny Tasks</h1>");
      changed = true;
    }
    if (nextHtml.includes("stops at 100") && !nextHtml.includes("list-limit")) {
      nextHtml = nextHtml.replace(/<span id="count">[^<]*<\/span>/, '<span id="count"></span>');
      nextHtml = nextHtml.replaceAll("Nothing here yet. Anyone with this link sees the same list. The list stops at 100 rows.", "Nothing here yet. Anyone with this link sees the same list.");
      nextHtml = nextHtml.replace(
        '<section class="file-shelf">',
        '<p class="list-limit">The list stops at 100 rows.</p>\n  <section class="file-shelf">',
      );
      if (!nextHtml.includes("list-limit")) {
        nextHtml = nextHtml.replace("</section>", '<p class="list-limit">The list stops at 100 rows.</p></section>');
      }
      changed = true;
    }
    const limitedCount = "count.textContent = tasks.length + (tasks.length === 1 ? ' item' : ' items') + '. The list stops at 100 rows.';";
    const plainCount = "count.textContent = tasks.length + (tasks.length === 1 ? ' item' : ' items');";
    if (nextScript.includes(limitedCount)) {
      nextScript = nextScript.replace(limitedCount, plainCount);
      changed = true;
    }
    if (nextScript.includes("Nothing here yet. Anyone with this link sees the same list. The list stops at 100 rows.")) {
      nextScript = nextScript.replaceAll(
        "Nothing here yet. Anyone with this link sees the same list. The list stops at 100 rows.",
        "Nothing here yet. Anyone with this link sees the same list.",
      );
      changed = true;
    }
    let nextCss = css;
    if (css.includes("prefers-color-scheme: dark") && (css.includes("box-shadow: 6px 6px 0 #000") || css.includes("box-shadow: 6px 6px 0 #f3efe5"))) {
      nextCss = css.replace("box-shadow: 6px 6px 0 #000", "border-color: #f3efe5; box-shadow: 4px 4px 0 #0a0c10").replace("box-shadow: 6px 6px 0 #f3efe5", "border-color: #f3efe5; box-shadow: 4px 4px 0 #0a0c10");
      if (!nextCss.includes(".composer input { color: #f3efe5; }")) {
        nextCss = nextCss.replace(
          ".composer { box-shadow: 6px 6px 0 #f3efe5; }",
          ".composer { box-shadow: 6px 6px 0 #f3efe5; }\n  .composer input { color: #f3efe5; }\n  .composer input::placeholder { color: #d9d4c8; }",
        );
      }
      nextCss = nextCss.replace(".empty, .task small { color: #b7b3a6; }", ".empty, .task small, .section-label { color: #f3efe5; }");
      changed = true;
    }
    if (nextCss.includes(".composer button") && !nextCss.includes("background: #c8f34a; color: #171910;")) {
      nextCss = nextCss.replace(
        /\.composer button \{[^}]*\}/,
        ".composer button { border: 0; background: #c8f34a; color: #171910; font-size: 25px; cursor: pointer; }",
      );
      changed = true;
    }
    if (nextCss.includes(".section-label label") && !nextCss.includes("background: #c8f34a; cursor: pointer; padding: 2px 6px;")) {
      nextCss = nextCss.replace(
        /\.section-label label \{[^}]*\}/,
        ".section-label label { color: #171910; background: #c8f34a; cursor: pointer; padding: 2px 6px; }",
      );
      changed = true;
    }
    if (!nextCss.includes(".list-limit")) {
      nextCss = nextCss.replace(".empty {", ".list-limit { margin: 8px 0 0; font-size: 13px; line-height: 1.4; }\n.empty {");
      changed = true;
    }
    if (changed) {
      return { ...project, files: { ...project.files, "index.html": nextHtml, "app.js": nextScript, "styles.css": nextCss } };
    }
  }
  if (!previousStarter) return project;
  const seed = cloneStarter();
  return {
    ...project,
    files: {
      ...project.files,
      "index.html": seed.files["index.html"],
      "styles.css": seed.files["styles.css"],
      "app.js": seed.files["app.js"],
    },
  };
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
