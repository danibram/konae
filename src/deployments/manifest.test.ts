import { describe, expect, test } from "bun:test";
import {
  createDefaultManifest,
  editorCapabilities,
  normalizeManifest,
} from "./manifest";

describe("deployment manifest", () => {
  test("preserves the app-user file policy and refuses invalid values", () => {
    expect(normalizeManifest({ fileAccess: "app-users" }).fileAccess).toBe("app-users");
    for (const fileAccess of [null, "private", "", 1]) {
      expect(() => normalizeManifest({ fileAccess })).toThrow('manifest.fileAccess must be "app-users" when set.');
    }
  });
  test("migrates projects created before manifest v1", () => {
    const manifest = normalizeManifest(undefined);
    expect(manifest.version).toBe(1);
    expect(manifest.visibility).toBe("private");
    expect(manifest.runtime.database.mode).toBe("declared-actions");
    expect(manifest.runtime.database.actions.map((action) => action.name)).toEqual([
      "tasks.list",
      "tasks.create",
      "tasks.toggle",
    ]);
  });

  test("a new project is private and only a manifest that says public is public", () => {
    expect(createDefaultManifest().visibility).toBe("private");
    expect(createDefaultManifest().runtime.auth.mode).toBe("required");
    for (const visibility of [undefined, "", "Public", "open", 1]) {
      const manifest = normalizeManifest({ ...createDefaultManifest(), visibility });
      expect(manifest.visibility).toBe("private");
      expect(manifest.runtime.auth.mode).toBe("required");
    }
    expect(normalizeManifest({ ...createDefaultManifest(), visibility: "public" }).visibility).toBe("public");
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
