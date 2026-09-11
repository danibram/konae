import { describe, expect, test } from "bun:test";
import { architectureProjectId } from "./architecture";
import { cloneStarter } from "./starter";
import { createWorkspaceProject, loadWorkspaceState, persistWorkspace, resetWorkspaceProject } from "./workspace";

function memoryStorage(seed: Record<string, string> = {}) {
  const values = new Map(Object.entries(seed));
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); },
    value: (key: string) => values.get(key),
  };
}

describe("workspace project registry", () => {
  test("migrates the original single-project draft and deployment", () => {
    const project = cloneStarter();
    const deployment = { id: "deploy_1", projectId: project.id, projectName: project.name, url: "http://tiny.localhost", createdAt: 1, manifest: project.manifest, accessCode: "secret" };
    const storage = memoryStorage({
      "littlebox:project": JSON.stringify(project),
      "littlebox:active-deployment": JSON.stringify(deployment),
    });
    const workspace = loadWorkspaceState(storage);

    expect(workspace.projects).toHaveLength(2);
    expect(workspace.projects.some((item) => item.id === architectureProjectId)).toBe(true);
    expect(workspace.activeProjectId).toBe(project.id);
    expect(workspace.deployments[project.id]?.url).toBe(deployment.url);
    expect(workspace.deployments[project.id]?.accessCode).toBeUndefined();
  });

  test("creates unique project ids and slugs", () => {
    const first = createWorkspaceProject("Weekend Planner", []);
    const second = createWorkspaceProject("Weekend Planner", [first]);
    expect(first.id).not.toBe(second.id);
    expect(first.slug).toBe("weekend-planner");
    expect(second.slug).toBe("weekend-planner-2");
  });

  test("seeds architecture as a data-free mini-app without duplicating it", () => {
    const first = loadWorkspaceState(memoryStorage());
    const storage = memoryStorage({ "littlebox:projects": JSON.stringify(first.projects) });
    const second = loadWorkspaceState(storage);
    const architecture = second.projects.find((project) => project.id === architectureProjectId);

    expect(second.projects.filter((project) => project.id === architectureProjectId)).toHaveLength(1);
    expect(architecture?.files["schema.sql"]).toBe("");
    expect(JSON.parse(architecture?.files["actions.json"] ?? "").actions).toEqual({});
    expect(architecture?.manifest.runtime.database.actions).toEqual([]);
  });

  test("resets architecture to its own source instead of the starter app", () => {
    const architecture = loadWorkspaceState(memoryStorage()).projects.find((project) => project.id === architectureProjectId);
    expect(architecture).toBeDefined();
    const reset = resetWorkspaceProject({ ...architecture!, files: { ...architecture!.files, "index.html": "changed" } });
    expect(reset.files["index.html"]).toContain("mapa del sistema");
    expect(reset.files["index.html"]).not.toContain("task-form");
  });

  test("persists the project index without private access codes", () => {
    const project = cloneStarter();
    const storage = memoryStorage();
    persistWorkspace([project], project.id, {
      [project.id]: { id: "deploy_1", projectId: project.id, projectName: project.name, url: "http://tiny.localhost", createdAt: 1, manifest: project.manifest, accessCode: "secret" },
    }, storage);
    expect(storage.value("littlebox:projects")).toContain(project.name);
    expect(storage.value("littlebox:deployments")).not.toContain("secret");
  });
});
