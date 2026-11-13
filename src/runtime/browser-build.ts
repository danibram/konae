import * as esbuild from "esbuild-wasm";
import wasmUrl from "esbuild-wasm/esbuild.wasm?url";
import type { MiniAppProject } from "../lib/types";
import { normalizeProjectPath } from "../lib/project-files";
import { hostedRuntime, type BuildArtifact } from "./artifact";
import { appEntry, assembleStaticArtifact } from "../lib/static-artifact.js";

let initialized: Promise<void> | undefined;

/**
 * The editor's door to the static publishing contract in
 * src/lib/static-artifact.js, the same rules `konae build` runs. A Managed
 * Project holds text files only, so a page that references an image the
 * project does not hold is refused here as it would be by the CLI.
 */
export async function buildProjectInBrowser(project: MiniAppProject): Promise<BuildArtifact> {
  return assembleStaticArtifact({
    paths: Object.keys(project.files),
    read: async (path) => project.files[path],
    // A Deploy minifies with NODE_ENV production, as konae build does; the preview stays readable, as konae dev does.
    compile: () => compileProjectInBrowser(project, { release: true }),
    bundle: async (path) => bundleInBrowser(project.files, path, true),
    runtime: hostedRuntime,
    routing: project.manifest.routing,
  });
}

export async function compileProjectInBrowser(project: MiniAppProject, options: { release?: boolean } = {}): Promise<{ body: string; styles: string; script: string }> {
  const entrypoint = appEntry(Object.keys(project.files)) ?? "app.js";
  const { script, styles: importedCss } = await bundleInBrowser(project.files, entrypoint, options.release ?? false);
  return {
    body: project.files["index.html"] ?? "<main id=app></main>",
    styles: [project.files["styles.css"] ?? "", importedCss].filter(Boolean).join("\n"),
    script,
  };
}

async function bundleInBrowser(files: Record<string, string>, entrypoint: string, release = false): Promise<{ script: string; styles: string }> {
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
    minify: release,
    define: { "process.env.NODE_ENV": JSON.stringify(release ? "production" : "development") },
    plugins: [projectFilesPlugin(files), packagesPlugin(declaredPackages(files))],
  });
  const script = result.outputFiles.find((file) => file.path.endsWith(".js"))?.text;
  if (!script) throw new Error("The browser builder did not produce JavaScript.");
  return { script, styles: result.outputFiles.find((file) => file.path.endsWith(".css"))?.text ?? "" };
}

function projectFilesPlugin(files: Record<string, string>): esbuild.Plugin {
  return {
    name: "konae-project-files",
    setup(build) {
      build.onResolve({ filter: /.*/ }, (args) => {
        if (args.namespace === "package") return undefined;
        // esbuild names an entry ./<path> when a file of that name sits in its working directory, as src/main.tsx does in this checkout.
        if (!args.importer) return { path: requireProjectFile(args.path.replace(/^\.\//, "")), namespace: "konae" };
        if (!args.path.startsWith(".")) return undefined;
        const parent = args.importer.includes("/") ? args.importer.slice(0, args.importer.lastIndexOf("/")) : "";
        const candidate = normalizeRelative(`${parent}/${args.path}`);
        const resolved = resolveExtension(candidate, files);
        if (!resolved) throw new Error(`Cannot resolve ${args.path} from ${args.importer}.`);
        return { path: resolved, namespace: "konae" };
      });
      build.onLoad({ filter: /.*/, namespace: "konae" }, (args) => {
        const contents = files[args.path];
        if (contents === undefined) throw new Error(`Project file not found: ${args.path}`);
        return { contents, loader: loaderFor(args.path), resolveDir: args.path.includes("/") ? args.path.slice(0, args.path.lastIndexOf("/")) : "" };
      });
    },
  };
}

/**
 * The CDN the editor's build reads packages from. The build fetches each
 * module as text and esbuild links it into the app's own bundle, so the
 * published app loads nothing from this origin and nothing fetched here runs
 * in the editor.
 */
export const packageOrigin = "https://esm.sh";

/** Exact versions such as 1.2.3 or 2.0.0-beta.1, so every build reads the same code. */
const exactVersion = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

/** The `dependencies` of the Capsule's package.json, the one list npm and the editor both read. */
export function declaredPackages(files: Record<string, string>): Map<string, string> {
  const source = files["package.json"];
  if (source === undefined) return new Map();
  let manifest: unknown;
  try { manifest = JSON.parse(source); } catch { throw new Error("package.json is not valid JSON."); }
  const dependencies = (manifest as { dependencies?: unknown } | null)?.dependencies ?? {};
  if (typeof dependencies !== "object" || dependencies === null || Array.isArray(dependencies)) throw new Error("package.json dependencies must be an object of package names and versions.");
  const result = new Map<string, string>();
  for (const [name, version] of Object.entries(dependencies)) {
    if (typeof version !== "string" || !exactVersion.test(version)) {
      throw new Error(`package.json asks for ${name} ${JSON.stringify(version)}. Name an exact version, such as "1.2.3", so every build gets the same code.`);
    }
    result.set(name, version);
  }
  return result;
}

/** The package name and the path inside it of a bare import such as `@scope/pkg/sub`. */
function splitSpecifier(specifier: string): { name: string; subpath: string } {
  const parts = specifier.split("/");
  const size = specifier.startsWith("@") ? 2 : 1;
  return { name: parts.slice(0, size).join("/"), subpath: parts.slice(size).join("/") };
}

/** The CDN URL of a bare import, pinned to the version package.json names. */
export function packageUrl(specifier: string, packages: Map<string, string>): string {
  if (specifier.startsWith("node:")) throw new Error(`${specifier} is a Node.js module. The app runs in a browser, so import a package that works there.`);
  const { name, subpath } = splitSpecifier(specifier);
  const version = packages.get(name);
  if (version === undefined) throw new Error(`${specifier} is not in package.json. Add "${name}": "<exact version>" to its dependencies.`);
  const path = `/${name}@${version}${subpath ? `/${subpath}` : ""}`;
  // A stylesheet, a JSON file or an asset is served as published; only modules take esm.sh's build options.
  if (/\.[a-z0-9]+$/i.test(subpath) && !/\.(?:m?js|cjs|jsx?|tsx?)$/i.test(subpath)) return `${packageOrigin}${path}`;
  // deps pins every declared package, so two packages that share one (a UI library and React, say) link the same copy.
  const deps = [...packages].filter(([other]) => other !== name).map(([other, pinned]) => `${other}@${pinned}`).join(",");
  return `${packageOrigin}${path}?target=es2022${deps ? `&deps=${deps}` : ""}`;
}

const fetchedModules = new Map<string, Promise<Uint8Array>>();

function fetchModule(url: string): Promise<Uint8Array> {
  let pending = fetchedModules.get(url);
  if (!pending) {
    pending = fetch(url).then(async (response) => {
      if (!response.ok) throw new Error(`${url} answered ${response.status}. Check the package name and version in package.json.`);
      return new Uint8Array(await response.arrayBuffer());
    }, (error) => {
      throw new Error(`The build could not reach ${packageOrigin} for ${url}: ${error instanceof Error ? error.message : error}. Check the connection and build again.`);
    });
    // A dropped connection is not cached, or every later build would fail too.
    pending.catch(() => fetchedModules.delete(url));
    fetchedModules.set(url, pending);
  }
  return pending;
}

/** A package's code and styles go in as source; a font or an image its CSS names goes in as a data: URL, which the published policy allows. */
function packageLoader(url: string): esbuild.Loader {
  const { pathname } = new URL(url);
  // A package root such as /react@19.0.0 ends in its version, so only a known asset extension counts.
  return /\.(?:woff2?|ttf|otf|eot|png|jpe?g|gif|svg|webp|avif|ico)$/i.test(pathname) ? "dataurl" : loaderFor(pathname);
}

/**
 * Resolves bare imports against package.json and fetches them, with the
 * modules they import in turn, from packageOrigin. Only that origin is read.
 */
function packagesPlugin(packages: Map<string, string>): esbuild.Plugin {
  return {
    name: "konae-packages",
    setup(build) {
      build.onResolve({ filter: /.*/ }, (args) => {
        if (args.namespace === "package") {
          if (args.path.startsWith("data:")) return { external: true };
          // In CSS, url(fonts/a.woff2) and @import are relative to the stylesheet even without ./
          const relative = args.kind === "url-token" || args.kind === "import-rule";
          const url = relative || args.path.startsWith("/") || args.path.startsWith(".") || /^https?:/.test(args.path)
            ? new URL(args.path, args.importer).href
            : packageUrl(args.path, packages);
          if (new URL(url).origin !== packageOrigin) throw new Error(`${args.importer} imports ${url}. The build reads packages only from ${packageOrigin}.`);
          return { path: url, namespace: "package" };
        }
        if (args.path.startsWith(".")) return undefined;
        if (/^[a-z][a-z0-9+.-]*:\/\//i.test(args.path)) throw new Error(`${args.importer} imports ${args.path}. Import a package by its name and list it in package.json.`);
        return { path: packageUrl(args.path, packages), namespace: "package" };
      });
      build.onLoad({ filter: /.*/, namespace: "package" }, async (args) => ({ contents: await fetchModule(args.path), loader: packageLoader(args.path) }));
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
