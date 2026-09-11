import { describe, expect, test } from "bun:test";
import {
  createDefaultManifest,
  createDeployment,
  editorCapabilities,
  normalizeManifest,
} from "./manifest";

describe("deployment manifest", () => {
  test("migrates projects created before manifest v1", () => {
    const manifest = normalizeManifest(undefined);
    expect(manifest.version).toBe(1);
    expect(manifest.visibility).toBe("public");
    expect(manifest.runtime.database.mode).toBe("declared-actions");
    expect(manifest.runtime.database.actions.map((action) => action.name)).toEqual([
      "tasks.list",
      "tasks.create",
      "tasks.toggle",
    ]);
  });

  test("private deployments force authentication without changing the project manifest", () => {
    const project = {
      id: "project_1",
      name: "Sensitive notes",
      slug: "sensitive-notes",
      manifest: createDefaultManifest(),
    };
    const deployment = createDeployment(project, "private", { id: "abcd-1234", now: 42 });

    expect(deployment.url).toBe("https://sensitive-notes-abcd.littlebox.app");
    expect(deployment.createdAt).toBe(42);
    expect(deployment.manifest.visibility).toBe("private");
    expect(deployment.manifest.runtime.auth.mode).toBe("required");
    expect(project.manifest.visibility).toBe("public");
    expect(project.manifest.runtime.auth.mode).toBe("optional");
  });

  test("keeps editor powers outside the runtime manifest", () => {
    const manifest = createDefaultManifest();
    const serialized = JSON.stringify(manifest);
    expect(editorCapabilities).toContain("database:export");
    expect(serialized).not.toContain("database:export");
    expect(serialized).not.toContain("database:query");
    expect(serialized).not.toContain("sql");
  });

  test("rejects duplicate declared actions", () => {
    const manifest = createDefaultManifest();
    manifest.runtime.database.actions.push({ ...manifest.runtime.database.actions[0] });
    expect(() => normalizeManifest(manifest)).toThrow("unique");
  });
});
