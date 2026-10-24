import { describe, expect, test } from "bun:test";
import { architectureProjectId } from "./architecture";
import { cloneStarter } from "./starter";
import { createWorkspaceProject, dismissDrafts, loadWorkspaceState, persistWorkspace, readDismissedDrafts, recoveryDrafts, resetWorkspaceProject, storedLocalProjects } from "./workspace";

function memoryStorage(seed: Record<string, string> = {}) {
  const values = new Map(Object.entries(seed));
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); },
    value: (key: string) => values.get(key),
  };
}

describe("workspace project registry", () => {
  test("an empty store offers nothing to import, not a starter or the architecture map", () => {
    expect(storedLocalProjects(memoryStorage())).toEqual([]);
    expect(storedLocalProjects(memoryStorage({ "littlebox:projects": "[]" }))).toEqual([]);
  });

  test("an untouched starter stored by an earlier build is not a draft", () => {
    expect(recoveryDrafts([cloneStarter(), { ...cloneStarter(), id: architectureProjectId }], [])).toEqual([]);
    const edited = { ...cloneStarter(), files: { ...cloneStarter().files, "app.js": "mine" } };
    expect(recoveryDrafts([edited], [])).toEqual([edited]);
  });

  test("a removed copy stays removed until it is edited again", () => {
    const storage = memoryStorage();
    const copy = { ...cloneStarter(), id: "app_copy", name: "Copy", updatedAt: 10 };
    dismissDrafts(storage, [copy]);
    expect(recoveryDrafts([copy], [], readDismissedDrafts(storage))).toEqual([]);
    const edited = { ...copy, updatedAt: 11 };
    expect(recoveryDrafts([edited], [], readDismissedDrafts(storage))).toEqual([edited]);
  });

  test("a stored project is offered once and the architecture map never", () => {
    const mine = { ...cloneStarter(), id: "app_mine", name: "Mine" };
    const found = storedLocalProjects(memoryStorage({ "littlebox:projects": JSON.stringify([mine, { ...cloneStarter(), id: architectureProjectId }]) }));
    expect(found.map((project) => project.id)).toEqual(["app_mine"]);
  });

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
  test("the seeded starter is not offered as drafts to import", () => {
    const seeded = loadWorkspaceState(memoryStorage()).projects;
    expect(seeded.map((project) => project.id).sort()).toEqual(["app_tiny_tasks"]);
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

    expect(workspace.projects).toHaveLength(1);
    expect(workspace.projects.some((item) => item.id === architectureProjectId)).toBe(false);
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

  test("drops a stored copy of the architecture map", () => {
    const mine = cloneStarter();
    const storage = memoryStorage({ "littlebox:projects": JSON.stringify([mine, { ...cloneStarter(), id: architectureProjectId }]) });
    expect(loadWorkspaceState(storage).projects.map((project) => project.id)).toEqual([mine.id]);
  });

  test("refreshes an unedited starter when its copy still hides the guest and stays light", () => {
    const stale = cloneStarter();
    stale.files["index.html"] = stale.files["index.html"].replace('class="ui-identity">Guest', 'class="identity">guest').replace('class="ui-page"', 'class="app-shell"');
    stale.files["styles.css"] = ":root { color-scheme: light; }";
    stale.files["app.js"] = stale.files["app.js"].replace("identity.textContent = session.role === 'anonymous' ? 'Guest' : session.name;", "identity.textContent = session.name;");
    const storage = memoryStorage({ "littlebox:projects": JSON.stringify([stale]) });
    const loaded = loadWorkspaceState(storage).projects.find((project) => project.id === stale.id);
    expect(loaded?.files["app.js"]).toContain("Nothing here yet");
    expect(loaded?.files["index.html"]).toContain(">Guest<");
    expect(loaded?.files["styles.css"]).toContain("prefers-color-scheme");
    expect(loaded?.files["app.js"]).toContain("Guest");
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
