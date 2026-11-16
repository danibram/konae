import { describe, expect, test } from "bun:test";
import {
  createDefaultManifest,
  editorCapabilities,
  normalizeManifest,
} from "./manifest";

describe("deployment manifest", () => {
  test("migrates projects created before manifest v1", () => {
    const manifest = normalizeManifest(undefined);
    expect(manifest.version).toBe(1);
    expect(manifest.visibility).toBe("private");
    expect(manifest).not.toHaveProperty("runtime");
  });

  test("a new project is private and only a manifest that says public is public", () => {
    expect(createDefaultManifest().visibility).toBe("private");
    for (const visibility of [undefined, "", "Public", "open", 1]) {
      const manifest = normalizeManifest({ ...createDefaultManifest(), visibility });
      expect(manifest.visibility).toBe("private");
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

  test("drops the obsolete runtime block when it normalizes a saved manifest", () => {
    const manifest = normalizeManifest({ version: 1, visibility: "public", runtime: { database: { actions: ["stale"] } } });
    expect(manifest.visibility).toBe("public");
    expect(manifest).not.toHaveProperty("runtime");
  });
});
