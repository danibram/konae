import { parseFunctionSources, withCronModules, validateFunctions, validateServerArtifact, validateFunctionSources, FUNCTIONS_ABI, type ServerArtifact } from "../kernel/functions";
import { refusal } from "../kernel/refusal";
import * as esbuild from "esbuild-wasm";
import wasmUrl from "esbuild-wasm/esbuild.wasm?url";
import type { MiniAppProject } from "../lib/types";
import { normalizeProjectPath } from "../lib/project-files";
import { sourceBytes, sourceContents, textFiles } from "../lib/source-files";
import { hostedRuntime, type BuildArtifact } from "./artifact";
import { appEntry, assembleStaticArtifact, resolveReference, servedPath } from "../lib/static-artifact.js";
import { sdkRequired } from "../lib/module-use";
import { nodeCompatBuildError, nodeCompatGlobals, nodeCompatPlugin } from "../lib/node-compat.js";
import { actionsModuleBody } from "../sdk/functions";

let initialized: Promise<void> | undefined;

export async function buildProjectInBrowser(project: MiniAppProject): Promise<BuildArtifact> {
  const manifest = parseFunctionSources(project.files["functions.json"], project.files["crons.json"]);
  const actions = JSON.parse(project.files["actions.json"] ?? '{"actions":{}}').actions ?? {};
  const modules = withCronModules(manifest, project.modules, actions);
  const context = { files: Object.keys(project.files), modules, actions };
  if (manifest) {
    validateFunctionSources(manifest, project.files);
    validateFunctions(manifest, context);
  }
  const artifact = await assembleStaticArtifact({
    paths: Object.keys(project.files),
    read: async (path) => sourceContents(project, path),
    compile: () => compileProjectInBrowser({ ...project, modules }, { release: true }),
    bundle: async (path) => bundleInBrowser(project, textFiles(project), path, true),
    runtime: hostedRuntime,
    sdk: sdkRequired(context.modules, project.files, project.manifest.visibility as string | undefined),
    routing: project.manifest.routing,
  });
  if (!manifest || !Object.keys(manifest.functions).length) return artifact;
  validateFunctions(manifest, { ...context, staticPaths: Object.keys(artifact.files) });
  const bundles: Record<string, string> = {};
  for (const [name, fn] of Object.entries(manifest.functions)) {
    const bundle = await bundleInBrowser(project, textFiles(project), fn.entry, true, true, name, actionsModuleBody());
    Object.defineProperty(bundles, name, { value: bundle.script, enumerable: true });
  }
  const serverArtifact: ServerArtifact = { version: 1, abi: FUNCTIONS_ABI, manifest, bundles };
  validateServerArtifact(manifest, serverArtifact);
  return { ...artifact, serverArtifact };
}

export async function compileProjectInBrowser(project: MiniAppProject, options: { release?: boolean } = {}): Promise<{ body: string; styles: string; script: string }> {
  const entrypoint = appEntry(Object.keys(project.files)) ?? "app.js";
  const bundled = await bundleInBrowser(project, textFiles(project), entrypoint, options.release ?? false);
  const compiled = {
    body: project.files["index.html"] ?? "<main id=app></main>",
    styles: [project.files["styles.css"] ?? "", bundled.styles].filter(Boolean).join("\n"),
    script: bundled.script,
  };
  await assembleStaticArtifact({
    paths: Object.keys(project.files),
    read: async (path) => sourceContents(project, path),
    compile: async () => ({ styles: compiled.styles, script: compiled.script }),
    bundle: async (path) => bundleInBrowser(project, textFiles(project), path, true),
    runtime: hostedRuntime,
    sdk: sdkRequired(project.modules, project.files, project.manifest.visibility as string | undefined),
    routing: project.manifest.routing,
  });
  return compiled;
}

async function bundleInBrowser(project: MiniAppProject, files: Record<string, string>, entrypoint: string, release = false, server = false, functionName = entrypoint, actionsSource?: string): Promise<{ script: string; styles: string }> {
  initialized ??= esbuild.initialize({ wasmURL: wasmUrl, worker: true }).catch((error) => {
    initialized = undefined;
    throw error;
  });
  await initialized;
  let result: esbuild.BuildResult;
  try { result = await esbuild.build({
    entryPoints: [entrypoint],
    bundle: true,
    write: false,
    format: "esm",
    platform: "browser",
    target: "es2022",
    outdir: "out",
    minify: release,
    metafile: true,
    logOverride: { "unsupported-dynamic-import": "error", "unsupported-require-call": "error" },
    ...(server ? { inject: [nodeCompatGlobals()], define: { process: "process", global: "globalThis" } }
      : { define: { "process.env.NODE_ENV": JSON.stringify(release ? "production" : "development") } }),
    plugins: [...(server ? [nodeCompatPlugin(), actionsVirtualPlugin(actionsSource ?? actionsModuleBody())] : []), projectFilesPlugin(files, project, server), packagesPlugin(declaredPackages(files))],
  }); } catch (error) { throw nodeCompatBuildError(error); }
  const outputFiles = result.outputFiles ?? [];
  if (server) {
    const output = Object.entries(result.metafile!.outputs).find(([file]) => file.endsWith(".js"));
      if (!output || output[1].imports.length || !output[1].exports.includes("handle") || outputFiles.length !== 1) {
      throw refusal("function.bundle-contract", `Function ${functionName} bundle must be a self-contained ESM module exporting handle.`);
    }
  }
  const script = outputFiles.find((file) => file.path.endsWith(".js"))?.text;
  if (!script) throw new Error("The browser builder did not produce JavaScript.");
  return { script, styles: outputFiles.find((file) => file.path.endsWith(".css"))?.text ?? "" };
}

function actionsVirtualPlugin(source: string): esbuild.Plugin {
  return { name: "konae-actions", setup(build) {
    build.onResolve({ filter: /^konae:actions$/ }, () => ({ path: "konae:actions", namespace: "konae-actions" }));
    build.onLoad({ filter: /.*/, namespace: "konae-actions" }, () => ({ contents: source, loader: "js" }));
  } };
}

function projectFilesPlugin(files: Record<string, string>, project: MiniAppProject, server: boolean): esbuild.Plugin {
  return {
    name: "konae-project-files",
    setup(build) {
      build.onResolve({ filter: /.*/ }, (args) => {
        if (args.namespace === "package") return undefined;
        if (args.kind === "url-token") {
          const importer = servedPath(args.importer) ?? args.importer;
          let target: string | null;
          try { target = resolveReference(args.path, importer); } catch { target = null; }
          if (target === null) return { path: args.path, external: true };
          const source = Object.keys(project.files).find((path) => path === target)
            ?? Object.keys(project.files).find((path) => servedPath(path) === target);
          if (!source) throw new Error(`${args.importer} references ${args.path}, but the Capsule holds no ${target}.`);
          if (!server && source.startsWith("functions/")) throw new Error("Server function sources cannot be imported by a browser entry.");
          const bytes = sourceBytes(project, source);
          return { path: `data:${mediaType(target)};base64,${bytesToBase64(bytes)}${fragmentSuffix(args.path)}`, external: true };
        }
        if (!args.importer) return { path: requireProjectFile(args.path.replace(/^\.\//, "")), namespace: "konae" };
        if (!args.path.startsWith(".")) return undefined;
        const parent = args.importer.includes("/") ? args.importer.slice(0, args.importer.lastIndexOf("/")) : "";
        const candidate = normalizeRelative(`${parent}/${args.path}`);
        const resolved = resolveExtension(candidate, files);
        if (!resolved) throw new Error(`Cannot resolve ${args.path} from ${args.importer}.`);
        return { path: resolved, namespace: "konae" };
      });
      build.onLoad({ filter: /.*/, namespace: "konae" }, (args) => {
        if (!server && args.path.startsWith("functions/")) throw new Error("Server function sources cannot be imported by a browser entry.");
        const contents = files[args.path];
        if (contents === undefined) throw new Error(`Project file not found: ${args.path}`);
        return { contents, loader: loaderFor(args.path), resolveDir: args.path.includes("/") ? args.path.slice(0, args.path.lastIndexOf("/")) : "" };
      });
    },
  };
}

function fragmentSuffix(raw: string): string {
  const fragmentAt = raw.indexOf("#");
  return fragmentAt === -1 ? "" : raw.slice(fragmentAt);
}

function mediaType(path: string): string {
  const extension = path.split(".").pop()?.toLowerCase();
  return ({ png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml", ico: "image/x-icon", avif: "image/avif", woff: "font/woff", woff2: "font/woff2", ttf: "font/ttf", otf: "font/otf", eot: "application/vnd.ms-fontobject" } as Record<string, string>)[extension ?? ""] ?? "application/octet-stream";
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let index = 0; index < bytes.length; index += 0x8000) binary += String.fromCharCode(...bytes.subarray(index, index + 0x8000));
  return btoa(binary);
}

export const packageOrigin = "https://esm.sh";

const exactVersion = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

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

function splitSpecifier(specifier: string): { name: string; subpath: string } {
  const parts = specifier.split("/");
  const size = specifier.startsWith("@") ? 2 : 1;
  return { name: parts.slice(0, size).join("/"), subpath: parts.slice(size).join("/") };
}

export function packageUrl(specifier: string, packages: Map<string, string>): string {
  if (specifier.startsWith("node:")) throw new Error(`${specifier} is a Node.js module. The app runs in a browser, so import a package that works there.`);
  const { name, subpath } = splitSpecifier(specifier);
  const version = packages.get(name);
  if (version === undefined) throw new Error(`${specifier} is not in package.json. Add "${name}": "<exact version>" to its dependencies.`);
  const path = `/${name}@${version}${subpath ? `/${subpath}` : ""}`;
  if (/\.[a-z0-9]+$/i.test(subpath) && !/\.(?:m?js|cjs|jsx?|tsx?)$/i.test(subpath)) return `${packageOrigin}${path}`;
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
    pending.catch(() => fetchedModules.delete(url));
    fetchedModules.set(url, pending);
  }
  return pending;
}

function packageLoader(url: string): esbuild.Loader {
  const { pathname } = new URL(url);
  return /\.(?:woff2?|ttf|otf|eot|png|jpe?g|gif|svg|webp|avif|ico)$/i.test(pathname) ? "dataurl" : loaderFor(pathname);
}

function packagesPlugin(packages: Map<string, string>): esbuild.Plugin {
  return {
    name: "konae-packages",
    setup(build) {
      build.onResolve({ filter: /.*/ }, (args) => {
        if (args.namespace === "package") {
          if (args.path.startsWith("data:")) return { external: true };
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
