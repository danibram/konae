import { describe, expect, test } from "bun:test";
import { architectureProjectId } from "./architecture";
import { cloneStarter } from "./starter";
import { createWorkspaceProject, loadWorkspaceState, persistWorkspace, recoveryDrafts, resetWorkspaceProject } from "./workspace";

function memoryStorage(seed: Record<string, string> = {}) {
  const values = new Map(Object.entries(seed));
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); },
    value: (key: string) => values.get(key),
  };
}

describe("workspace project registry", () => {
  test("local copies with another timestamp or deployed revision are not extra projects", () => {
    const saved = cloneStarter();
    const cached = { ...saved, updatedAt: saved.updatedAt - 100, databaseModel: { ...saved.databaseModel, revision: "old-browser-revision" } };
    expect(recoveryDrafts([cached], [saved])).toEqual([]);
  });

  test("recovery keeps one newest unsaved source per project without changing storage", () => {
    const saved = cloneStarter();
    const first = { ...saved, updatedAt: 2, files: { ...saved.files, "app.js": "first unsaved change" } };
    const last = { ...saved, updatedAt: 3, files: { ...saved.files, "app.js": "latest unsaved change" } };
    expect(recoveryDrafts([first, last, first], [saved])).toEqual([last]);
    expect(first.files["app.js"]).toBe("first unsaved change");
  });
  test("the seeded starter and architecture map are not offered as drafts to import", () => {
    const seeded = loadWorkspaceState(memoryStorage()).projects;
    expect(seeded.map((project) => project.id).sort()).toEqual(["app_littlebox_architecture", "app_tiny_tasks"]);
    expect(recoveryDrafts(seeded, [])).toEqual([]);
    const edited = { ...seeded[0], files: { ...seeded[0].files, "app.js": "an owner edit" } };
    expect(recoveryDrafts([edited], [])).toEqual([edited]);
  });

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

  test("refreshes an unedited starter when its copy still hides the guest and stays light", () => {
    const stale = cloneStarter();
    stale.files["index.html"] = stale.files["index.html"].replace('class="identity">Guest', 'class="identity">guest');
    stale.files["styles.css"] = ":root { color-scheme: light; }";
    stale.files["app.js"] = stale.files["app.js"].replace("identity.textContent = session.role === 'anonymous' ? 'Guest' : session.name;", "identity.textContent = session.name;");
    const storage = memoryStorage({ "littlebox:projects": JSON.stringify([stale]) });
    const loaded = loadWorkspaceState(storage).projects.find((project) => project.id === stale.id);
    expect(loaded?.files["app.js"]).toContain("Nothing here yet");
    expect(loaded?.files["index.html"]).toContain(">Guest<");
    expect(loaded?.files["styles.css"]).toContain("prefers-color-scheme");
    expect(loaded?.files["app.js"]).toContain("Guest");
  });

  test("resets architecture to its own source instead of the starter app", () => {
    const architecture = loadWorkspaceState(memoryStorage()).projects.find((project) => project.id === architectureProjectId);
    expect(architecture).toBeDefined();
    const reset = resetWorkspaceProject({ ...architecture!, files: { ...architecture!.files, "index.html": "changed" } });
    expect(reset.files["index.html"]).toContain("system map");
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

  test("repairs a starter that cleared the chosen file before sign-in finished", () => {
    const broken = cloneStarter();
    broken.files["app.js"] = broken.files["app.js"].replace(
      `if ((await miniapp.auth.session()).role === 'anonymous') {
    await miniapp.auth.signIn();
    const session = await miniapp.auth.session();
    if (session.role === 'anonymous') {
      fileInput.value = '';
      return;
    }
    identity.textContent = session.name;
  }`,
      `if ((await miniapp.auth.session()).role === 'anonymous') {
    fileInput.value = '';
    miniapp.auth.signIn();
    return;
  }`,
    );
    expect(broken.files["app.js"]).toContain("fileInput.value = '';\n    miniapp.auth.signIn();");
    const storage = memoryStorage({ "littlebox:projects": JSON.stringify([broken]) });
    const loaded = loadWorkspaceState(storage).projects.find((project) => project.id === broken.id);
    expect(loaded?.files["app.js"]).toContain("await miniapp.auth.signIn();");
    expect(loaded?.files["app.js"]).toContain("if (session.role === 'anonymous') {\n      fileInput.value = '';\n      return;\n    }");
    expect(loaded?.files["app.js"]).not.toContain("fileInput.value = '';\n    miniapp.auth.signIn();");
  });
});
