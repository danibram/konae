import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createDefaultManifest } from "../deployments/manifest";
import { createHostedArtifact, type BuildArtifact } from "../runtime/artifact";
import type { MiniAppProject } from "./types";
import { buildProjectExport, type ExportFile } from "./export-project-kit";
import { kernelFiles as bundledKernelFiles } from "./export-project-sources";

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
  files: { "index.html": "<!doctype html><script data-littlebox-runtime>window.miniapp={}</script>" },
};

const kernelFiles: ExportFile[] = [
  { path: "kernel-rs/Cargo.toml", contents: "[package]\nname='kernel'" },
  { path: "kernel-rs/src/main.rs", contents: "fn main() {}" },
];

describe("buildProjectExport", () => {
  test("includes every module declared by the exported Kernel", () => {
    const files = readTar(buildProjectExport(project, artifact, bundledKernelFiles).archive);
    const main = files.get("kernel-rs/src/main.rs") ?? "";
    const modules = [...main.matchAll(/^mod (\w+);/gm)].map((match) => match[1]);
    expect(modules.length).toBeGreaterThan(0);
    for (const module of modules) {
      expect(files.get(`kernel-rs/src/${module}.rs`)).toBeTruthy();
    }
  });

  test("includes every file the exported Kernel build reads with include_str!", () => {
    const files = readTar(buildProjectExport(project, artifact, bundledKernelFiles).archive);
    const sources = [...files].filter(([path]) => path.startsWith("kernel-rs/src/") && path.endsWith(".rs"));
    // Test modules read fixtures a release build never compiles.
    const built = sources.map(([, source]) => source.split(/#\[cfg\(test\)\]\s*mod tests/)[0]);
    const included = built.flatMap((source) => [...source.matchAll(/include_str!\("([^"]+)"\)/g)].map((match) => match[1]));
    expect(included).toContain("sdk-guide.json");
    for (const file of included) {
      expect(files.get(`kernel-rs/src/${file}`)).toBeTruthy();
    }
  });

  test("a failed self-hosted deploy releases its transfer and owner session", async () => {
    const calls: string[] = [];
    const server = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      async fetch(request) {
        const pathname = new URL(request.url).pathname;
        if (pathname === "/_littlebox/owner/login") return Response.json({}, { headers: { "set-cookie": "owner=test; Path=/" } });
        if (pathname === "/_littlebox/owner/logout") { calls.push("logout"); return Response.json({}); }
        const message = await request.json() as { id?: string; method: string; params?: { name: string } };
        if (!message.id) return new Response(null, { status: 202 });
        const name = message.params?.name;
        if (name) calls.push(name);
        if (name === "release_plan") return Response.json({ jsonrpc: "2.0", id: message.id, result: { isError: true, content: [{ type: "text", text: "Invalid schema" }] } });
        const result = name === "project_inspect" ? {} : name === "transfer_begin" ? { transferId: "transfer-1", chunkBytes: 65536 } : {};
        return Response.json({ jsonrpc: "2.0", id: message.id, result: { structuredContent: { result } } });
      },
    });
    const directory = await mkdtemp(path.join(tmpdir(), "littlebox-deploy-failure-"));
    try {
      const { entries } = buildProjectExport(project, artifact, bundledKernelFiles);
      for (const entry of entries) {
        const target = path.join(directory, entry.path);
        await mkdir(path.dirname(target), { recursive: true });
        await writeFile(target, entry.contents);
      }
      await writeFile(path.join(directory, ".env"), `LITTLEBOX_PORT=${server.port}\nLITTLEBOX_ADMIN_KEY=test-owner-password\n`);
      const deploy = Bun.spawn(["node", "scripts/deploy.mjs"], { cwd: directory, stdout: "pipe", stderr: "pipe" });
      const [exitCode, stderr] = await Promise.all([deploy.exited, new Response(deploy.stderr).text()]);
      expect(exitCode).not.toBe(0);
      expect(stderr).toContain("Invalid schema");
      expect(calls.slice(-3)).toEqual(["release_plan", "transfer_cancel", "logout"]);
      expect(calls).not.toContain("release_apply");
    } finally {
      server.stop(true);
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("packages editable source, the static artifact, and a local Kernel bootstrap", () => {
    const { archive, entries } = buildProjectExport(project, artifact, kernelFiles);
    const files = readTar(archive);
    expect(files.get("app/src/main.ts")).toBe("console.log('app')");
    expect(files.get("app/schema.ts")).toBe("export const notes = {};");
    expect(files.get("kernel-rs/src/main.rs")).toBe("fn main() {}");
    expect(files.get("Dockerfile")).toContain("cargo build --release --locked");
    expect(files.get("Dockerfile")).toContain("useradd");
    expect(files.get("Dockerfile")).toContain("littlebox-entrypoint");
    expect(files.get("entrypoint.sh")).toContain("setpriv");
    expect(files.get("compose.yaml")).toContain("127.0.0.1:");
    expect(files.get("start.sh")).toContain("docker compose up -d --build");
    expect(files.get("scripts/deploy.mjs")).toContain("tools/call");
    expect(files.get("scripts/deploy.mjs")).toContain("littlebox/deployment.json");
    expect(files.get("scripts/deploy.mjs")).toContain("if (!remote.deployment)");
    expect(() => new Bun.Transpiler({ loader: "js" }).transformSync(files.get("scripts/build.mjs") ?? "")).not.toThrow();
    expect(() => new Bun.Transpiler({ loader: "js" }).transformSync(files.get("scripts/deploy.mjs") ?? "")).not.toThrow();
    const exportedProject = JSON.parse(files.get("littlebox/project.json") ?? "{}") as Record<string, unknown>;
    expect(exportedProject.id).toBe("app_notes");
    expect(files.get("littlebox/deploy.json")).toBeUndefined();
    expect(entries.find((entry) => entry.path === "start.sh")?.mode).toBe(0o755);
    expect(files.get("package.json")).toContain('"start": "sh start.sh"');
  });

  test("carries the Action Contract of its actions.json, and refuses one that does not parse", () => {
    const withAction = { ...project, files: { ...project.files, "actions.json": JSON.stringify({ version: 1, actions: {
      "notes.list": { kind: "query", access: "authenticated", statement: "SELECT id FROM notes LIMIT 20", bindings: [] },
    } }) } };
    const files = readTar(buildProjectExport(withAction, artifact, kernelFiles).archive);
    const contract = JSON.parse(files.get("littlebox/action-contract.json")!);
    expect(contract).toMatchObject({ format: "littlebox-action-contract", project: { id: "app_notes", name: "Private Notes" } });
    expect(contract.actions[0].output).toEqual({ kind: "rows", columns: [{ name: "id", type: "integer", nullable: false, from: { table: "notes", column: "id" } }], unnamed: [] });
    expect(files.get("littlebox/action-contract.md")).toContain("## notes.list");
    expect(JSON.parse(files.get("littlebox/action-contract.schema.json")!).title).toBe("Littlebox Action Contract");
    expect(files.get("README.md")).toContain("littlebox/action-contract.json");
    expect(() => buildProjectExport({ ...project, files: { ...project.files, "actions.json": "{" } }, artifact, kernelFiles))
      .toThrow("The export cannot describe its Server Actions: actions.json must contain valid JSON.");
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

  test("exports the signup policy for the first self-hosted deployment", () => {
    const policy = { mode: "allowlist" as const, allowlist: ["writer@example.test", "@team.example.test"] };
    const files = readTar(buildProjectExport(project, artifact, kernelFiles, {
      initialDatabase: new Uint8Array(), initialSignupPolicy: policy,
    }).archive);
    expect(JSON.parse(files.get("littlebox/initial-signup-policy.json")!)).toEqual(policy);
    expect(files.get("scripts/deploy.mjs")).toContain("payload.initialSignupPolicy = JSON.parse");
  });

  test("the exported manifest keeps the snapshot visibility when local source says public", () => {
    const files = readTar(buildProjectExport({ ...project, manifest: { ...project.manifest, visibility: "public" } }, artifact, kernelFiles, {
      initialDatabase: new Uint8Array(), visibility: "private",
    }).archive);
    expect(JSON.parse(files.get("littlebox/project.json")!).manifest.visibility).toBe("private");
  });

  test("portable state is private after extraction and excluded from Git", async () => {
    const { archive, entries } = buildProjectExport(project, artifact, kernelFiles, {
      initialDatabase: new Uint8Array([1]), initialIdentity: new Uint8Array([2]), initialFiles: new Uint8Array([3]),
      initialSignupPolicy: { mode: "closed", allowlist: [] },
    });
    const privatePaths = entries.filter((entry) => entry.path.startsWith("littlebox/initial-")).map((entry) => entry.path);
    expect(privatePaths).toHaveLength(4);
    let offset = 0;
    while (offset + 512 <= archive.length && archive[offset] !== 0) {
      const name = readString(archive, offset, 100);
      const size = Number.parseInt(readString(archive, offset + 124, 12), 8);
      if (privatePaths.includes(name)) expect(Number.parseInt(readString(archive, offset + 100, 8), 8)).toBe(0o600);
      offset += 512 + Math.ceil(size / 512) * 512;
    }
    const directory = await mkdtemp(path.join(tmpdir(), "littlebox-export-ignore-"));
    try {
      await writeFile(path.join(directory, ".gitignore"), readTar(archive).get(".gitignore")!);
      const init = Bun.spawn(["git", "-c", "init.defaultBranch=main", "init", "-q", directory], { stdout: "ignore", stderr: "pipe" });
      expect(await init.exited).toBe(0);
      const ignored = Bun.spawn(["git", "check-ignore", ...privatePaths, "littlebox/artifact.json", "app/app.js"], { cwd: directory, stdout: "pipe", stderr: "pipe" });
      const names = (await new Response(ignored.stdout).text()).trim().split("\n");
      expect(await ignored.exited).toBe(0);
      expect(names).toEqual(privatePaths);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
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
  });

  test("a private project exports as private, and deploys private when the manifest says nothing", () => {
    const privateExport = readTar(buildProjectExport({ ...project, manifest: { ...project.manifest, visibility: "private" } }, artifact, kernelFiles).archive);
    const exported = JSON.parse(privateExport.get("littlebox/project.json") ?? "{}") as { manifest: { visibility: string } };
    expect(exported.manifest.visibility).toBe("private");
    // The same default `littlebox publish` uses.
    expect(privateExport.get("scripts/deploy.mjs")).toContain("visibility: project.manifest.visibility || 'private'");
  });

  test("refuses an artifact whose runtime script is not marked", () => {
    const unmarked: BuildArtifact = { ...artifact, files: { "index.html": "<!doctype html><script>window.miniapp={}</script>" } };
    expect(() => buildProjectExport(project, unmarked, kernelFiles)).toThrow("does not mark its Littlebox runtime");
  });

  /**
   * The bug this pins: the build script took the first bare <script> in the
   * artifact as the runtime, but the owner's index.html comes first, so a body
   * with its own script replaced the SDK on every rebuild.
   */
  test("the exported build script rebuilds the artifact with the runtime, not the owner's script", async () => {
    const body = "<main id=app></main><script>window.ownerInline = true;</script>";
    const source = {
      ...project,
      files: { ...project.files, "index.html": body, "src/main.ts": "const message: string = 'rebuilt';\nconsole.log(message);" },
    };
    const hosted = createHostedArtifact({ body, styles: "", script: "console.log('first build')" });
    const { entries } = buildProjectExport(source, hosted, kernelFiles);

    const directory = await mkdtemp(path.join(tmpdir(), "littlebox-export-"));
    try {
      for (const entry of entries) {
        const target = path.join(directory, entry.path);
        await mkdir(path.dirname(target), { recursive: true });
        await writeFile(target, entry.contents);
      }
      // Node ESM resolves esbuild from the folder's own node_modules; the repo's pinned copy stands in for npm install.
      await symlink(path.resolve(import.meta.dir, "../../node_modules"), path.join(directory, "node_modules"), "dir");
      const build = Bun.spawn(["node", "scripts/build.mjs"], { cwd: directory, stdout: "pipe", stderr: "pipe" });
      const [exitCode, stderr] = await Promise.all([build.exited, new Response(build.stderr).text()]);
      expect(stderr).toBe("");
      expect(exitCode).toBe(0);

      const rebuilt = JSON.parse(await readFile(path.join(directory, "littlebox/artifact.json"), "utf8")) as BuildArtifact;
      const html = rebuilt.files["index.html"];
      expect(html).toContain("rebuilt");
      expect(html).toContain(body);
      const runtime = html.match(/<script data-littlebox-runtime>([\s\S]*?)<\/script>/)?.[1] ?? "";
      expect(runtime).toContain("/_littlebox/mcp");
      expect(html.indexOf("window.ownerInline")).toBeLessThan(html.indexOf("<script data-littlebox-runtime>"));
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  }, 30_000);
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

