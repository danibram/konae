import { describe, expect, test } from "bun:test";
import { checksum } from "../mcp/client";
import { checkEjectPackage } from "./eject-package";

const encoder = new TextEncoder();

async function packaged(files: Record<string, string>, contents?: { path: string; size: number; sha256: string }[]) {
  const listed = contents ?? await Promise.all(Object.entries(files).map(async ([path, text]) => ({ path, size: encoder.encode(text).byteLength, sha256: await checksum(encoder.encode(text)) })));
  const bytes = encoder.encode(JSON.stringify({ format: "konae-eject", version: 1, projectId: "app_one", sourcesRevision: 3, contents: listed, directories: ["src"], files }));
  return { bytes, plan: { projectId: "app_one", expectedRevision: 3, package: { format: "konae-eject" as const, size: bytes.byteLength, sha256: await checksum(bytes), contents: listed } } };
}

describe("checkEjectPackage", () => {
  test("returns the files of a package that matches its plan", async () => {
    const { bytes, plan } = await packaged({ "app.js": "console.log(1)", "src/main.ts": "export {}" });
    expect(await checkEjectPackage(bytes, plan)).toEqual({ files: { "app.js": "console.log(1)", "src/main.ts": "export {}" }, directories: ["src"] });
  });

  test("refuses bytes that differ from the planned SHA-256", async () => {
    const { bytes, plan } = await packaged({ "app.js": "a" });
    const tampered = bytes.slice(); tampered[tampered.length - 3] ^= 1;
    await expect(checkEjectPackage(tampered, plan)).rejects.toThrow("does not match its plan");
  });

  test("refuses a file that differs from the content list", async () => {
    const { bytes, plan } = await packaged({ "app.js": "a" }, [{ path: "app.js", size: 1, sha256: "0".repeat(64) }]);
    await expect(checkEjectPackage(bytes, plan)).rejects.toThrow("app.js does not match");
  });

  test("refuses a path that leaves the folder", async () => {
    const { bytes, plan } = await packaged({ "../outside.js": "a" });
    await expect(checkEjectPackage(bytes, plan)).rejects.toThrow("invalid file");
  });

  test("refuses a package for another revision", async () => {
    const { bytes, plan } = await packaged({ "app.js": "a" });
    await expect(checkEjectPackage(bytes, { ...plan, expectedRevision: 2 })).rejects.toThrow("another project or revision");
  });
});
