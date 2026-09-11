import { describe, expect, test } from "bun:test";
import { createDefaultManifest } from "../deployments/manifest";
import type { BuildArtifact } from "../runtime/artifact";
import type { MiniAppProject } from "./types";
import { buildProjectExport, type ExportFile } from "./export-project-kit";

const project: MiniAppProject = {
  id: "app_notes",
  name: "Private Notes",
  slug: "private-notes",
  manifest: createDefaultManifest(),
  updatedAt: 1,
  databaseModel: { source: "export const notes = {};", acceptedSource: "export const notes = {};", revision: "rev_1" },
  directories: ["src"],
  files: {
    "index.html": "<main id=app></main>",
    "src/main.ts": "console.log('app')",
    "schema.sql": "CREATE TABLE notes(id INTEGER PRIMARY KEY);",
    "actions.json": "{\"version\":1,\"actions\":{}}",
  },
};

const artifact: BuildArtifact = {
  version: 1,
  entrypoint: "index.html",
  files: { "index.html": "<!doctype html><script>window.miniapp={}</script>" },
};

const kernelFiles: ExportFile[] = [
  { path: "kernel-rs/Cargo.toml", contents: "[package]\nname='kernel'" },
  { path: "kernel-rs/src/main.rs", contents: "fn main() {}" },
];

describe("buildProjectExport", () => {
  test("packages editable source, the static artifact, and a local Kernel bootstrap", () => {
    const { archive, entries } = buildProjectExport(project, artifact, kernelFiles);
    const files = readTar(archive);
    expect(files.get("app/src/main.ts")).toBe("console.log('app')");
    expect(files.get("app/schema.ts")).toBe("export const notes = {};");
    expect(files.get("kernel-rs/src/main.rs")).toBe("fn main() {}");
    expect(files.get("Dockerfile")).toContain("cargo build --release --locked");
    expect(files.get("start.sh")).toContain("docker compose up -d --build");
    expect(files.get("scripts/deploy.mjs")).toContain("/_littlebox/admin/deploy");
    expect(files.get("scripts/deploy.mjs")).toContain("littlebox/deployment.json");
    expect(files.get("scripts/deploy.mjs")).toContain("Portable state can only be imported on the first deployment.");
    expect(() => new Bun.Transpiler({ loader: "js" }).transformSync(files.get("scripts/build.mjs") ?? "")).not.toThrow();
    expect(() => new Bun.Transpiler({ loader: "js" }).transformSync(files.get("scripts/deploy.mjs") ?? "")).not.toThrow();
    const exportedProject = JSON.parse(files.get("littlebox/project.json") ?? "{}") as Record<string, unknown>;
    expect(exportedProject.id).toBe("app_notes");
    expect(files.get("littlebox/deploy.json")).toBeUndefined();
    expect(entries.find((entry) => entry.path === "start.sh")?.mode).toBe(0o755);
    expect(files.get("package.json")).toContain('"start": "sh start.sh"');
  });

  test("includes only an explicitly supplied SQLite snapshot", () => {
    const snapshot = Uint8Array.from([0, 1, 2, 255]);
    const { archive, entries } = buildProjectExport(project, artifact, kernelFiles, snapshot);
    const files = readTar(archive);
    const database = entries.find((entry) => entry.path === "littlebox/initial-database.sqlite")?.contents;
    expect(files.has("littlebox/initial-database.sqlite")).toBe(true);
    expect(database).toBeInstanceOf(Uint8Array);
    expect(database).toEqual(snapshot);
  });

  test("rejects portable state whose combined size exceeds the Kernel limit", () => {
    const state = {
      initialDatabase: new Uint8Array(8 * 1024 * 1024),
      initialIdentity: new Uint8Array(8 * 1024 * 1024),
      initialFiles: new Uint8Array(1),
    };
    expect(() => buildProjectExport(project, artifact, kernelFiles, state)).toThrow("Combined portable state exceeds the 16 MiB");
  });

  test("rejects path traversal and snapshots larger than the Kernel import limit", () => {
    expect(() => buildProjectExport({ ...project, files: { "../secret": "x" } }, artifact, kernelFiles)).toThrow("Invalid project file path");
    expect(() => buildProjectExport(project, artifact, kernelFiles, new Uint8Array(16 * 1024 * 1024 + 1))).toThrow("16 MiB");
    const privateExport = readTar(buildProjectExport({ ...project, manifest: { ...project.manifest, visibility: "private" } }, artifact, kernelFiles).archive);
    expect(privateExport.get("scripts/deploy.mjs")).toContain("visibility: project.manifest.visibility || 'public'");
  });
});

function readTar(archive: Uint8Array): Map<string, string> {
  const result = new Map<string, string>();
  const decoder = new TextDecoder();
  let offset = 0;
  while (offset + 512 <= archive.length && archive[offset] !== 0) {
    const header = archive.subarray(offset, offset + 512);
    const name = readString(header, 0, 100);
    const prefix = readString(header, 345, 155);
    const path = prefix ? `${prefix}/${name}` : name;
    const size = Number.parseInt(readString(header, 124, 12).trim().replace(/\0.*$/, ""), 8);
    const start = offset + 512;
    result.set(path, decoder.decode(archive.subarray(start, start + size)));
    offset = start + Math.ceil(size / 512) * 512;
  }
  return result;
}

function readString(bytes: Uint8Array, offset: number, width: number) {
  const field = bytes.subarray(offset, offset + width);
  const end = field.indexOf(0);
  return new TextDecoder().decode(end === -1 ? field : field.subarray(0, end));
}

