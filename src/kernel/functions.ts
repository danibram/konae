import { refusal } from "./refusal";
import { checksum } from "../mcp/client";
import { parse as parseEsm } from "@babel/parser";

export const FUNCTIONS_ABI = "konae-functions/1";
export type FunctionDefinition = { entry: string; mount: string; methods: ("GET" | "POST")[]; access: "authenticated" | "anonymous"; actions: string[] };
export type FunctionsManifest = { version: 1; functions: Record<string, FunctionDefinition> };
export type ServerArtifact = { version: 1; abi: typeof FUNCTIONS_ABI; manifest: FunctionsManifest; bundles: Record<string, string> };
export type FunctionsContext = { files: Iterable<string>; modules?: Record<string, { on?: boolean }>; actions?: Record<string, unknown>; staticPaths?: string[] };

type BundleAstNode = { type: string; [key: string]: unknown };

function bundleAstNode(value: unknown): value is BundleAstNode {
  return value !== null && typeof value === "object" && !Array.isArray(value) && typeof (value as { type?: unknown }).type === "string";
}

function bundleBindingHasHandle(value: unknown): boolean {
  const pending = [value];
  while (pending.length) {
    const current = pending.pop();
    if (!bundleAstNode(current)) continue;
    if (current.type === "Identifier" && current.name === "handle") return true;
    if (current.type === "RestElement") pending.push(current.argument);
    if (current.type === "AssignmentPattern") pending.push(current.left);
    if (current.type === "ArrayPattern" && Array.isArray(current.elements)) {
      for (const element of current.elements) pending.push(element);
    }
    if (current.type === "ObjectPattern" && Array.isArray(current.properties)) {
      for (const property of current.properties) {
        if (bundleAstNode(property)) pending.push(property.type === "RestElement" ? property.argument : property.value);
      }
    }
  }
  return false;
}

function bundleExportedHandle(value: unknown): boolean {
  if (!bundleAstNode(value) || value.type !== "ExportNamedDeclaration") return false;
  const declaration = value.declaration;
  if (bundleAstNode(declaration)) {
    if ((declaration.type === "FunctionDeclaration" || declaration.type === "ClassDeclaration") && bundleBindingHasHandle(declaration.id)) return true;
    if (declaration.type === "VariableDeclaration" && Array.isArray(declaration.declarations)
      && declaration.declarations.some((item) => bundleAstNode(item) && bundleBindingHasHandle(item.id))) return true;
  }
  return Array.isArray(value.specifiers) && value.specifiers.some((specifier) => {
    if (!bundleAstNode(specifier) || !bundleAstNode(specifier.exported)) return false;
    const exported = specifier.exported;
    return exported.type === "Identifier" && exported.name === "handle"
      || exported.type === "StringLiteral" && exported.value === "handle";
  });
}

function validServerBundle(source: string): boolean {
  let program: unknown;
  try { program = parseEsm(source, { sourceType: "module", createImportExpressions: true }).program; }
  catch { return false; }
  let exportsHandle = false;
  const pending = [program];
  while (pending.length) {
    const current = pending.pop();
    if (Array.isArray(current)) { for (const item of current) pending.push(item); continue; }
    if (!bundleAstNode(current)) continue;
    if (current.type === "ImportDeclaration" || current.type === "ExportAllDeclaration"
      || current.type === "ExportNamedDeclaration" && current.source !== null && current.source !== undefined
      || current.type === "ImportExpression"
      || current.type === "CallExpression" && bundleAstNode(current.callee) && current.callee.type === "Import") return false;
    if (bundleExportedHandle(current)) exportsHandle = true;
    for (const child of Object.values(current)) pending.push(child);
  }
  return exportsHandle;
}

function functionsContract(refuse: (code: Parameters<typeof refusal>[0], message: string) => Error = (code, message) => Object.assign(new Error(message), { code })) {
  const FUNCTIONS_ABI = "konae-functions/1";
  const refusal = refuse;
function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function byteOrder(left: string, right: string): number {
  const encoder = new TextEncoder();
  const a = encoder.encode(left), b = encoder.encode(right);
  for (let index = 0; index < Math.min(a.length, b.length); index++) {
    if (a[index] !== b[index]) return a[index] - b[index];
  }
  return a.length - b.length;
}
function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (object(value)) return `{${Object.keys(value).sort(byteOrder).map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  return JSON.stringify(value);
}
function keys(value: Record<string, unknown>, allowed: string[], label: string) {
  const key = Object.keys(value).sort(byteOrder).find((key) => !allowed.includes(key));
  if (key !== undefined) throw refusal("function.key-unknown", `${label} has unknown key ${key}.`);
}
function functionPathsOverlap(a: string, b: string): boolean {
  return a === b || a.startsWith(`${b}/`) || b.startsWith(`${a}/`);
}
function parseFunctions(source: string): FunctionsManifest {
  let value: unknown;
  try { value = JSON.parse(source); } catch { value = null; }
  if (!object(value) || value.version !== 1 || !object(value.functions)) throw refusal("function.invalid", "functions.json must be a JSON object with version 1 and a functions object.");
  keys(value, ["version", "functions"], "functions.json");
  if (Object.keys(value.functions).length > 32) throw refusal("function.count-over", "functions.json may declare at most 32 functions.");
  const functions: Record<string, FunctionDefinition> = {};
  for (const name of Object.keys(value.functions).sort(byteOrder)) {
    const declaration = value.functions[name];
    if (!/^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(name)) throw refusal("function.name-invalid", `Function ${name} must have a name of 1 to 64 ASCII letters, digits, underscores or hyphens, starting with a letter.`);
    if (!object(declaration)) throw refusal("function.definition-invalid", `Function ${name} must be an object.`);
    keys(declaration, ["entry", "mount", "methods", "access", "actions"], `Function ${name}`);
    const { entry, mount, methods } = declaration;
    if (typeof entry !== "string" || !entry.startsWith("functions/") || entry.includes("\\") || entry.split("/").some((part) => !part || part === "." || part === "..") || !/\.(ts|js|mts|mjs)$/.test(entry)) throw refusal("function.entry-invalid", `Function ${name} entry must name a .ts, .js, .mts or .mjs file under functions/.`);
    if (typeof mount !== "string" || mount.length > 256 || !/^\/[A-Za-z0-9_-]+(?:\/[A-Za-z0-9_-]+)*$/.test(mount)) throw refusal("function.mount-invalid", `Function ${name} mount must be an absolute path of nonempty ASCII letter, digit, underscore or hyphen segments.`);
    if (["/_konae", "/oauth", "/.well-known", "/server", "/functions", "/mcp", "/assets"].some((path) => functionPathsOverlap(mount, path))) throw refusal("function.mount-reserved", `Function ${name} mount ${mount} is reserved.`);
    if (!Array.isArray(methods) || !methods.length || !methods.every((method): method is "GET" | "POST" => method === "GET" || method === "POST") || new Set(methods).size !== methods.length) throw refusal("function.methods-invalid", `Function ${name} methods must be a nonempty array of GET or POST without duplicates.`);
    const access = declaration.access === undefined ? "authenticated" : declaration.access;
    if (access !== "authenticated" && access !== "anonymous") throw refusal("function.access-invalid", `Function ${name} access must be authenticated or anonymous.`);
    const actions = declaration.actions === undefined ? [] : declaration.actions;
    if (!Array.isArray(actions) || !actions.every((action): action is string => typeof action === "string" && action.length > 0 && new TextEncoder().encode(action).length <= 128) || new Set(actions).size !== actions.length) throw refusal("function.actions-invalid", `Function ${name} actions must be an array of unique action names.`);
    Object.defineProperty(functions, name, { value: { entry, mount, methods: [...methods].sort(byteOrder), access, actions: [...actions].sort(byteOrder) }, enumerable: true });
  }
  const manifest: FunctionsManifest = { version: 1, functions };
  const hashes = Object.fromEntries(Object.keys(functions).map((name) => [name, "0".repeat(64)]));
  if (new TextEncoder().encode(serverManifestText(manifest, hashes)).length > 262144) throw refusal("function.manifest-size", "Server manifest exceeds 262144 UTF-8 bytes.");
  return manifest;
}

function validateFunctions(manifest: FunctionsManifest, context: FunctionsContext): void {
  const entries = Object.entries(manifest.functions).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
  const files = new Set(context.files);
  for (const [name, fn] of entries) if (!files.has(fn.entry)) throw refusal("function.entry-missing", `Function ${name} entry ${fn.entry} is missing from the app sources.`);
  if (entries.length && context.modules?.functions?.on !== true) throw refusal("module.off", "Module functions is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run.");
  for (const required of ["auth", "actions"] as const) {
    const needed = entries.some(([, fn]) => required === "auth" ? fn.access === "authenticated" : fn.actions.length > 0);
    if (needed && context.modules?.[required]?.on !== true) throw refusal("module.requires", `Module functions requires ${required}, which is off. Turn ${required} on first.`);
  }
  for (const [name, fn] of entries) {
    for (const action of fn.actions) {
      const definition = context.actions && Object.hasOwn(context.actions, action) ? context.actions[action] : undefined;
      if (definition === undefined || definition === null) throw refusal("function.action-missing", `Function ${name} names unknown action ${action}.`);
      const thenTarget = Object.values(context.actions ?? {}).some((other) => object(other) && other.kind === "call" && object(other.then) && other.then.action === action);
      if (!object(definition) || !["query", "mutation"].includes(String(definition.kind)) || definition.access === "granted" || thenTarget || ["notify", "call", "signal", "schedule", "subscribe", "http", "grants"].some((key) => Object.hasOwn(definition, key))) throw refusal("function.action-unsupported", `Function ${name} action ${action} must be a query or mutation without external effects.`);
    }
    for (const [other, declaration] of entries) {
      if (other >= name) break;
      if (functionPathsOverlap(fn.mount, declaration.mount)) throw refusal("function.mount-conflict", `Function ${name} mount ${fn.mount} overlaps function ${other}.`);
    }
    const paths = [...context.staticPaths ?? []];
    for (const action of Object.values(context.actions ?? {})) {
      if (!object(action)) continue;
      for (const field of ["http", "page"]) {
        const route = action[field];
        if (object(route) && typeof route.path === "string") paths.push(route.path);
      }
    }
    for (const raw of paths.sort(byteOrder)) {
      const path = `/${raw.replace(/^\/+/, "")}`;
      if (functionPathsOverlap(fn.mount, path.split("/:")[0])) throw refusal("function.mount-conflict", `Function ${name} mount ${fn.mount} overlaps route ${path}.`);
    }
  }
}

function validateServerArtifactEnvelope(manifest: FunctionsManifest | undefined, input: unknown): ServerArtifact | undefined {
  if (input === undefined || input === null) {
    if (manifest && Object.keys(manifest.functions).length) throw refusal("function.artifact-missing", "Declared functions require a Server Artifact.");
    return undefined;
  }
  if (!object(input) || input.version !== 1 || input.abi !== FUNCTIONS_ABI) throw refusal("function.artifact-abi", "Server Artifact requires version 1 and ABI konae-functions/1.");
  keys(input, ["version", "abi", "manifest", "bundles"], "Server Artifact");
  const normalized = parseFunctions(JSON.stringify(input.manifest));
  if (canonical(input.manifest) !== canonical(normalized)) throw refusal("function.artifact-manifest", "Server Artifact manifest does not match functions.json.");
  if (!object(input.bundles) || JSON.stringify(Object.keys(input.bundles).sort(byteOrder)) !== JSON.stringify(Object.keys(normalized.functions).sort(byteOrder))) throw refusal("function.artifact-bundles", "Server Artifact bundles must match the declared function names.");
  const bundles: Record<string, string> = {};
  let total = 0;
  for (const name of Object.keys(input.bundles).sort(byteOrder)) {
    const bundle = input.bundles[name];
    const length = typeof bundle === "string" ? new TextEncoder().encode(bundle).length : 0;
    if (typeof bundle !== "string" || length === 0 || length > 1048576) throw refusal("function.bundle-size", `Function ${name} bundle must contain 1 to 1048576 UTF-8 bytes.`);
    total += length;
    Object.defineProperty(bundles, name, { value: bundle, enumerable: true });
  }
  if (total > 8388608) throw refusal("function.artifact-size", "Server Artifact exceeds 8388608 UTF-8 bytes.");
  if (!manifest || canonical(normalized) !== canonical(manifest)) throw refusal("function.artifact-manifest", "Server Artifact manifest does not match functions.json.");
  return { version: 1, abi: FUNCTIONS_ABI, manifest, bundles };
}

function serverManifestText(manifest: FunctionsManifest, hashes: Record<string, string>): string {
  return JSON.stringify({ version: 1, abi: FUNCTIONS_ABI, manifest, hashes }, null, 2);
}

  return { parseFunctions, validateFunctions, validateServerArtifactEnvelope, serverManifestText };
}

const { parseFunctions, validateFunctions, validateServerArtifactEnvelope, serverManifestText } = functionsContract(refusal);
export { parseFunctions, validateFunctions, serverManifestText };

export function validateServerArtifact(manifest: FunctionsManifest | undefined, input: unknown): ServerArtifact | undefined {
  const artifact = validateServerArtifactEnvelope(manifest, input);
  if (!artifact) return undefined;
  for (const [name, bundle] of Object.entries(artifact.bundles)) {
    if (!validServerBundle(bundle)) throw refusal("function.bundle-contract", `Function ${name} bundle must be a self-contained ESM module exporting handle.`);
  }
  return artifact;
}

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export async function serverFolderEntries(artifact: ServerArtifact): Promise<{ path: string; contents: string }[]> {
  validateServerArtifact(artifact.manifest, artifact);
  const hashes = Object.fromEntries(await Promise.all(Object.entries(artifact.bundles).map(async ([name, bundle]) => [name, await checksum(new TextEncoder().encode(bundle))])));
  return [
    { path: "server/manifest.json", contents: serverManifestText(artifact.manifest, hashes) },
    ...Object.entries(artifact.bundles).map(([name, contents]) => ({ path: `server/${name}.mjs`, contents })),
  ];
}

export function serverArtifactFromFolder(files: Record<string, Uint8Array>): ServerArtifact | undefined {
  if (!Object.keys(files).length) return undefined;
  const decode = new TextDecoder("utf-8", { fatal: true });
  if (!files["manifest.json"] || files["manifest.json"].length > 262144) throw Error("Invalid server manifest file.");
  const published: unknown = JSON.parse(decode.decode(files["manifest.json"]));
  if (!object(published)) throw Error("Invalid server manifest file.");
  const manifest = parseFunctions(JSON.stringify(published.manifest));
  const expected = ["manifest.json", ...Object.keys(manifest.functions).map((name) => `${name}.mjs`)].sort();
  if (JSON.stringify(Object.keys(files).sort()) !== JSON.stringify(expected)) throw Error("Server directory contains a missing or undeclared file.");
  const bundles = Object.fromEntries(Object.keys(manifest.functions).map((name) => [name, decode.decode(files[`${name}.mjs`])]));
  return validateServerArtifact(manifest, { version: published.version, abi: published.abi, manifest: published.manifest, bundles });
}

export async function verifyServerFolder(files: Record<string, Uint8Array>): Promise<void> {
  const artifact = serverArtifactFromFolder(files);
  if (!artifact) return;
  const published: unknown = JSON.parse(new TextDecoder().decode(files["manifest.json"]));
  if (!object(published) || !object(published.hashes)) throw Error("Server Artifact hashes do not match its bundles.");
  const hashes = published.hashes;
  if (Object.keys(hashes).length !== Object.keys(artifact.bundles).length) throw Error("Server Artifact hashes do not match its bundles.");
  for (const name of Object.keys(artifact.bundles)) if (hashes[name] !== await checksum(files[`${name}.mjs`])) throw Error("Server Artifact hashes do not match its bundles.");
}

export function functionsBuildSupport(): string {
  return `const { parseFunctions, validateFunctions, validateServerArtifactEnvelope } = (${functionsContract.toString()})();`;
}
