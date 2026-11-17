import { describe, expect, test } from "bun:test";
import { checkPackageManifest, packageComponentLimits } from "./portable-package";

const manifest = () => ({
  format: "konae-package", version: 1, packageId: "p", createdAt: 1, deploymentId: "d", projectId: "app",
  visibility: "private", publishedRevision: 1, excluded: [],
  contents: [{ name: "database", size: 4096, sha256: "a".repeat(64) }],
});

describe("package manifests", () => {
  test("accept version 1 with a database", () => {
    expect(checkPackageManifest(manifest()).contents[0].name).toBe("database");
  });

  test("a package without actions may omit its database", () => {
    const staticPackage = { ...manifest(), contents: [], modules: { actions: { on: false } } };
    expect(checkPackageManifest(staticPackage).contents).toEqual([]);
    expect(() => checkPackageManifest({ ...staticPackage, modules: {} })).toThrow("The package manifest lists no database. This Kernel requires a database component for apps with actions on.");
  });

  test("refuse what the Kernel refuses, with the Kernel's reason", () => {
    const cases: [string, (value: ReturnType<typeof manifest>) => unknown][] = [
      ["Package version 3 is not supported", (value) => ({ ...value, version: 3 })],
      ["Package version 2 is not supported", (value) => ({ ...value, version: 2 })],
      ["not a Konae package manifest", (value) => ({ ...value, format: "zip" })],
      ["Unknown package component secrets", (value) => ({ ...value, contents: [...value.contents, { name: "secrets", size: 1, sha256: "a".repeat(64) }] })],
      ["twice", (value) => ({ ...value, contents: [...value.contents, value.contents[0]] })],
      ["its limit is", (value) => ({ ...value, contents: [{ ...value.contents[0], size: packageComponentLimits.database + 1 }] })],
      ["its limit is", (value) => ({ ...value, contents: [...value.contents, { name: "identity", size: packageComponentLimits.identity + 1, sha256: "b".repeat(64) }] })],
      ["lists no database", (value) => ({ ...value, contents: [] })],
      ["no valid SHA-256", (value) => ({ ...value, contents: [{ ...value.contents[0], sha256: "A".repeat(64) }] })],
      ["no valid visibility", (value) => ({ ...value, visibility: "secret" })],
    ];
    for (const [message, change] of cases) expect(() => checkPackageManifest(change(manifest()))).toThrow(message);
  });
});
