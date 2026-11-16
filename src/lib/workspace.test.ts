import { describe, expect, test } from "bun:test";
import { createWorkspaceProject, loadWorkspaceState, persistWorkspace, resetWorkspaceProject } from "./workspace";
import { cloneStarter } from "./starter";

function memoryStorage(seed: Record<string, string> = {}): Storage {
  const values = new Map(Object.entries(seed));
  return {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => { values.set(key, String(value)); },
    removeItem: (key) => { values.delete(key); },
    clear: () => { values.clear(); },
    key: (index) => [...values.keys()][index] ?? null,
    get length() { return values.size; },
  } as Storage;
}

describe("workspace browser state", () => {
  test("ignores old browser source copies and preserves them without applying them", () => {
    const stale = { ...cloneStarter(), name: "Stale browser copy", files: { "index.html": "stale" } };
    const storage = memoryStorage({ "konae:projects": JSON.stringify([stale]), "konae:project": JSON.stringify(stale) });
    const loaded = loadWorkspaceState(storage);
    expect(loaded.projects.map((project) => project.name)).not.toContain("Stale browser copy");
    expect(storage.getItem("konae:projects")).toBe(JSON.stringify([stale]));
  });

  test("stores selected app and deployment receipts without source files", () => {
    const storage = memoryStorage();
    const project = createWorkspaceProject("Reading club", []);
    persistWorkspace([project], project.id, {}, storage);
    expect(storage.getItem("konae:active-project")).toBe(project.id);
    expect(storage.getItem("konae:deployments")).toBe("{}");
    expect(storage.getItem("konae:projects")).toBeNull();
  });

  test("creates unique slugs and resets only the selected in-memory project", () => {
    const first = createWorkspaceProject("Reading club", []);
    const second = createWorkspaceProject("Reading club", [first]);
    expect(second.slug).toBe("reading-club-2");
    expect(resetWorkspaceProject(first).id).toBe(first.id);
  });
});
