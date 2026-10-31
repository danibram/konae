import { describe, expect, test } from "bun:test";
import { checkPackageManifest, packageComponentLimits } from "./portable-package";

const manifest = () => ({
  format: "konae-package", version: 1, packageId: "p", createdAt: 1, deploymentId: "d", projectId: "app",
  visibility: "private", publishedRevision: 1, excluded: [],
  contents: [{ name: "database", size: 4096, sha256: "a".repeat(64) }],
});

// The same cases as `manifests_name_their_version_components_and_limits` in kernel-rs/src/package.rs.
describe("package manifests", () => {
  test("accept version 1 with a database", () => {
    expect(checkPackageManifest(manifest()).contents[0].name).toBe("database");
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
