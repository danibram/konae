import * as esbuild from "esbuild-wasm";
import wasmUrl from "esbuild-wasm/esbuild.wasm?url";
import type { MiniAppProject } from "../lib/types";
import { normalizeProjectPath } from "../lib/project-files";
import { hostedRuntime, type BuildArtifact } from "./artifact";
import { assembleStaticArtifact } from "../lib/static-artifact.js";

let initialized: Promise<void> | undefined;

/**
 * The editor's door to the static publishing contract in
 * src/lib/static-artifact.js, the same rules `littlebox build` runs. A Managed
 * Project holds text files only, so a page that references an image the
 * project does not hold is refused here as it would be by the CLI.
 */
export async function buildProjectInBrowser(project: MiniAppProject): Promise<BuildArtifact> {
  return assembleStaticArtifact({
    paths: Object.keys(project.files),
    read: async (path) => project.files[path],
    compile: () => compileProjectInBrowser(project),
    bundle: async (path) => bundleInBrowser(project.files, path),
    runtime: hostedRuntime,
    routing: project.manifest.routing,
  });
}

export async function compileProjectInBrowser(project: MiniAppProject): Promise<{ body: string; styles: string; script: string }> {
  const entrypoint = project.files["src/main.ts"] !== undefined ? "src/main.ts" : "app.js";
  const { script, styles: importedCss } = await bundleInBrowser(project.files, entrypoint);
  return {
    body: project.files["index.html"] ?? "<main id=app></main>",
    styles: [project.files["styles.css"] ?? "", importedCss].filter(Boolean).join("\n"),
    script,
  };
}

async function bundleInBrowser(files: Record<string, string>, entrypoint: string): Promise<{ script: string; styles: string }> {
  // A failed start (a wasm fetch that dropped, say) is not cached, or every later build would fail too.
  initialized ??= esbuild.initialize({ wasmURL: wasmUrl, worker: true }).catch((error) => {
    initialized = undefined;
    throw error;
  });
  await initialized;
  const result = await esbuild.build({
    entryPoints: [entrypoint],
    bundle: true,
    write: false,
    format: "esm",
    platform: "browser",
    target: "es2022",
    outdir: "out",
    plugins: [projectFilesPlugin(files)],
  });
  const script = result.outputFiles.find((file) => file.path.endsWith(".js"))?.text;
  if (!script) throw new Error("The browser builder did not produce JavaScript.");
  return { script, styles: result.outputFiles.find((file) => file.path.endsWith(".css"))?.text ?? "" };
}

function projectFilesPlugin(files: Record<string, string>): esbuild.Plugin {
  return {
    name: "littlebox-project-files",
    setup(build) {
      build.onResolve({ filter: /.*/ }, (args) => {
        if (!args.importer) return { path: requireProjectFile(args.path), namespace: "littlebox" };
        if (!args.path.startsWith(".")) throw new Error(`Package imports are not enabled in Managed Projects: ${args.path}`);
        const parent = args.importer.includes("/") ? args.importer.slice(0, args.importer.lastIndexOf("/")) : "";
        const candidate = normalizeRelative(`${parent}/${args.path}`);
        const resolved = resolveExtension(candidate, files);
        if (!resolved) throw new Error(`Cannot resolve ${args.path} from ${args.importer}.`);
        return { path: resolved, namespace: "littlebox" };
      });
      build.onLoad({ filter: /.*/, namespace: "littlebox" }, (args) => {
        const contents = files[args.path];
        if (contents === undefined) throw new Error(`Project file not found: ${args.path}`);
        return { contents, loader: loaderFor(args.path), resolveDir: args.path.includes("/") ? args.path.slice(0, args.path.lastIndexOf("/")) : "" };
      });
    },
  };
}

function requireProjectFile(path: string) {
  const normalized = normalizeProjectPath(path);
  if (!normalized) throw new Error(`Invalid project path: ${path}`);
  return normalized;
}

function normalizeRelative(path: string): string {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") parts.pop(); else parts.push(part);
  }
  return parts.join("/");
}

function resolveExtension(path: string, files: Record<string, string>): string | null {
  const candidates = [path, `${path}.ts`, `${path}.tsx`, `${path}.js`, `${path}.jsx`, `${path}.css`, `${path}/index.ts`, `${path}/index.js`];
  return candidates.find((candidate) => files[candidate] !== undefined) ?? null;
}

function loaderFor(path: string): esbuild.Loader {
  const extension = path.split(".").at(-1)?.toLowerCase();
  if (extension === "ts") return "ts";
  if (extension === "tsx") return "tsx";
  if (extension === "jsx") return "jsx";
  if (extension === "css") return "css";
  if (extension === "json") return "json";
  return "js";
}
