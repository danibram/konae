import { spawnSync } from 'node:child_process';
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { appEntries, appEntry, assembleStaticArtifact } from './static-artifact.mjs';
import { bundleModule, bundleFunctions } from './local-builder.mjs';
import { createHash } from 'node:crypto';

const sdkRequired = function sdkRequired(modules, sources, visibility) {
  const sourceFiles = Symbol.iterator in Object(sources) ? void 0 : sources, files = sourceFiles ? Object.values(sourceFiles) : Array.from(sources), entries = sourceFiles ? Object.entries(sourceFiles) : files.map((source) => ["", source]), sdkMentions = entries.some(([name, source]) => name !== "actions.json" && name !== "schema.sql" && /\bminiapp\b/.test(source)), browserCall = (name) => files.some((source) => new RegExp(`(?:miniapp\\s*\\.\\s*${name}\\b|miniapp\\s*\\[\\s*["']${name}["']\\s*\\]|window\\s*\\[\\s*["']miniapp["']\\s*\\]\\s*\\.\\s*${name}\\b|window\\s*\\[\\s*["']miniapp["']\\s*\\]\\s*\\[\\s*["']${name}["']\\s*\\])`).test(source)), on = (name, inferred) => modules?.[name] ? modules[name].on === !0 : inferred, actionsUsed = entries.some(([name, source]) => {
    if (source.includes("miniapp.actions"))
      return !0;
    if (name === "schema.sql" && /\bCREATE\s+(?:(?:TEMP(?:ORARY)?|UNIQUE)\s+)*(?:TABLE|INDEX)\b/i.test(source))
      return !0;
    if (name !== "actions.json" && name !== "")
      return !1;
    try {
      const value = JSON.parse(source), actions = value && typeof value === "object" && !Array.isArray(value) ? value.actions : void 0;
      return actions !== null && typeof actions === "object" && !Array.isArray(actions) && Object.keys(actions).length > 0;
    } catch {
      return !1;
    }
  }), authUsed = files.some((source) => source.includes("miniapp.auth") || source.includes("miniapp.files") || source.includes("/_konae/files/") || /"access"\s*:\s*"authenticated"/.test(source) || /"source"\s*:\s*"actor"/.test(source)), filesUsed = files.some((source) => source.includes("miniapp.files") || source.includes("/_konae/files/")), moduleUsesSdk = on("actions", actionsUsed) || on("auth", visibility === "private" || authUsed) || on("files", filesUsed);
  return browserCall("page") || moduleUsesSdk && sdkMentions;
};
const { parseFunctions, validateFunctions, validateServerArtifact } = (function functionsContract(refuse = (code, message) => Object.assign(Error(message), { code })) {
  const refusal = refuse;
  function object(value) {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }
  function byteOrder(left, right) {
    const encoder = new TextEncoder, a = encoder.encode(left), b = encoder.encode(right);
    for (let index = 0;index < Math.min(a.length, b.length); index++)
      if (a[index] !== b[index])
        return a[index] - b[index];
    return a.length - b.length;
  }
  function canonical(value) {
    if (Array.isArray(value))
      return `[${value.map(canonical).join(",")}]`;
    if (object(value))
      return `{${Object.keys(value).sort(byteOrder).map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
    return JSON.stringify(value);
  }
  function keys(value, allowed, label) {
    const key = Object.keys(value).sort(byteOrder).find((key) => !allowed.includes(key));
    if (key !== void 0)
      throw refusal("function.key-unknown", `${label} has unknown key ${key}.`);
  }
  function functionPathsOverlap(a, b) {
    return a === b || a.startsWith(`${b}/`) || b.startsWith(`${a}/`);
  }
  function parseFunctions(source) {
    let value;
    try {
      value = JSON.parse(source);
    } catch {
      value = null;
    }
    if (!object(value) || value.version !== 1 || !object(value.functions))
      throw refusal("function.invalid", "functions.json must be a JSON object with version 1 and a functions object.");
    keys(value, ["version", "functions"], "functions.json");
    if (Object.keys(value.functions).length > 32)
      throw refusal("function.count-over", "functions.json may declare at most 32 functions.");
    const functions = {};
    for (const name of Object.keys(value.functions).sort(byteOrder)) {
      const declaration = value.functions[name];
      if (!/^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(name))
        throw refusal("function.name-invalid", `Function ${name} must have a name of 1 to 64 ASCII letters, digits, underscores or hyphens, starting with a letter.`);
      if (!object(declaration))
        throw refusal("function.definition-invalid", `Function ${name} must be an object.`);
      keys(declaration, ["entry", "mount", "methods", "access", "actions"], `Function ${name}`);
      const { entry, mount, methods } = declaration;
      if (typeof entry !== "string" || !entry.startsWith("functions/") || entry.includes("\\") || entry.split("/").some((part) => !part || part === "." || part === "..") || !/\.(ts|js|mts|mjs)$/.test(entry))
        throw refusal("function.entry-invalid", `Function ${name} entry must name a .ts, .js, .mts or .mjs file under functions/.`);
      if (typeof mount !== "string" || mount.length > 256 || !/^\/[A-Za-z0-9_-]+(?:\/[A-Za-z0-9_-]+)*$/.test(mount))
        throw refusal("function.mount-invalid", `Function ${name} mount must be an absolute path of nonempty ASCII letter, digit, underscore or hyphen segments.`);
      if (["/_konae", "/oauth", "/.well-known", "/server", "/functions", "/mcp", "/assets"].some((path) => functionPathsOverlap(mount, path)))
        throw refusal("function.mount-reserved", `Function ${name} mount ${mount} is reserved.`);
      if (!Array.isArray(methods) || !methods.length || !methods.every((method) => method === "GET" || method === "POST") || new Set(methods).size !== methods.length)
        throw refusal("function.methods-invalid", `Function ${name} methods must be a nonempty array of GET or POST without duplicates.`);
      const access = declaration.access === void 0 ? "authenticated" : declaration.access;
      if (access !== "authenticated" && access !== "anonymous")
        throw refusal("function.access-invalid", `Function ${name} access must be authenticated or anonymous.`);
      const actions = declaration.actions === void 0 ? [] : declaration.actions;
      if (!Array.isArray(actions) || !actions.every((action) => typeof action === "string" && action.length > 0 && new TextEncoder().encode(action).length <= 128) || new Set(actions).size !== actions.length)
        throw refusal("function.actions-invalid", `Function ${name} actions must be an array of unique action names.`);
      Object.defineProperty(functions, name, { value: { entry, mount, methods: [...methods].sort(byteOrder), access, actions: [...actions].sort(byteOrder) }, enumerable: !0 });
    }
    const manifest = { version: 1, functions }, hashes = Object.fromEntries(Object.keys(functions).map((name) => [name, "0".repeat(64)]));
    if (new TextEncoder().encode(serverManifestText(manifest, hashes)).length > 262144)
      throw refusal("function.manifest-size", "Server manifest exceeds 262144 UTF-8 bytes.");
    return manifest;
  }
  function validateFunctions(manifest, context) {
    const entries = Object.entries(manifest.functions).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0), files = new Set(context.files);
    for (const [name, fn] of entries)
      if (!files.has(fn.entry))
        throw refusal("function.entry-missing", `Function ${name} entry ${fn.entry} is missing from the app sources.`);
    if (entries.length && context.modules?.functions?.on !== !0)
      throw refusal("module.off", "Module functions is off. Turn it on with module_set or App settings for a published app, or in the folder's konae.json for a local run.");
    for (const required of ["auth", "actions"])
      if (entries.some(([, fn]) => required === "auth" ? fn.access === "authenticated" : fn.actions.length > 0) && context.modules?.[required]?.on !== !0)
        throw refusal("module.requires", `Module functions requires ${required}, which is off. Turn ${required} on first.`);
    for (const [name, fn] of entries) {
      for (const action of fn.actions) {
        const definition = context.actions && Object.hasOwn(context.actions, action) ? context.actions[action] : void 0;
        if (definition === void 0 || definition === null)
          throw refusal("function.action-missing", `Function ${name} names unknown action ${action}.`);
        const thenTarget = Object.values(context.actions ?? {}).some((other) => object(other) && other.kind === "call" && object(other.then) && other.then.action === action);
        if (!object(definition) || !["query", "mutation"].includes(String(definition.kind)) || definition.access === "granted" || thenTarget || ["notify", "call", "signal", "schedule", "subscribe", "http", "grants"].some((key) => Object.hasOwn(definition, key)))
          throw refusal("function.action-unsupported", `Function ${name} action ${action} must be a query or mutation without external effects.`);
      }
      for (const [other, declaration] of entries) {
        if (other >= name)
          break;
        if (functionPathsOverlap(fn.mount, declaration.mount))
          throw refusal("function.mount-conflict", `Function ${name} mount ${fn.mount} overlaps function ${other}.`);
      }
      const paths = [...context.staticPaths ?? []];
      for (const action of Object.values(context.actions ?? {})) {
        if (!object(action))
          continue;
        for (const field of ["http", "page"]) {
          const route = action[field];
          if (object(route) && typeof route.path === "string")
            paths.push(route.path);
        }
      }
      for (const raw of paths.sort(byteOrder)) {
        const path = `/${raw.replace(/^\/+/, "")}`;
        if (functionPathsOverlap(fn.mount, path.split("/:")[0]))
          throw refusal("function.mount-conflict", `Function ${name} mount ${fn.mount} overlaps route ${path}.`);
      }
    }
  }
  function validateServerArtifact(manifest, input) {
    if (input === void 0 || input === null) {
      if (manifest && Object.keys(manifest.functions).length)
        throw refusal("function.artifact-missing", "Declared functions require a Server Artifact.");
      return;
    }
    if (!object(input) || input.version !== 1 || input.abi !== "konae-functions/1")
      throw refusal("function.artifact-abi", "Server Artifact requires version 1 and ABI konae-functions/1.");
    keys(input, ["version", "abi", "manifest", "bundles"], "Server Artifact");
    const normalized = parseFunctions(JSON.stringify(input.manifest));
    if (canonical(input.manifest) !== canonical(normalized))
      throw refusal("function.artifact-manifest", "Server Artifact manifest does not match functions.json.");
    if (!object(input.bundles) || JSON.stringify(Object.keys(input.bundles).sort(byteOrder)) !== JSON.stringify(Object.keys(normalized.functions).sort(byteOrder)))
      throw refusal("function.artifact-bundles", "Server Artifact bundles must match the declared function names.");
    const bundles = {};
    let total = 0;
    for (const name of Object.keys(input.bundles).sort(byteOrder)) {
      const bundle = input.bundles[name], length = typeof bundle === "string" ? new TextEncoder().encode(bundle).length : 0;
      if (typeof bundle !== "string" || length === 0 || length > 1048576)
        throw refusal("function.bundle-size", `Function ${name} bundle must contain 1 to 1048576 UTF-8 bytes.`);
      total += length;
      Object.defineProperty(bundles, name, { value: bundle, enumerable: !0 });
    }
    if (total > 8388608)
      throw refusal("function.artifact-size", "Server Artifact exceeds 8388608 UTF-8 bytes.");
    if (!manifest || canonical(normalized) !== canonical(manifest))
      throw refusal("function.artifact-manifest", "Server Artifact manifest does not match functions.json.");
    return { version: 1, abi: "konae-functions/1", manifest, bundles };
  }
  function serverManifestText(manifest, hashes) {
    return JSON.stringify({ version: 1, abi: "konae-functions/1", manifest, hashes }, null, 2);
  }
  return { parseFunctions, validateFunctions, validateServerArtifact, serverManifestText };
})();

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const app = path.join(root, 'app');
const dist = path.join(root, 'dist');
const development = process.argv.includes('--dev');
const settings = JSON.parse(await readFile(path.join(root, 'konae.json'), 'utf8'));
const runtime = await readFile(path.join(root, 'scripts/runtime.js'), 'utf8');
const paths = await walk('');
const sources = await Promise.all(paths.map(async (file) => [file, await readFile(path.join(app, file), 'utf8').catch(() => '')]));
const actionsSource = await readFile(path.join(app, 'actions.json'), 'utf8').catch(() => '');
const schemaSource = await readFile(path.join(app, 'schema.sql'), 'utf8').catch(() => '');
const sdk = sdkRequired(settings.modules, Object.fromEntries(sources), settings.visibility);
const functionSource = Object.fromEntries(sources)['functions.json'];
const functions = functionSource === undefined ? undefined : parseFunctions(functionSource);
const functionContext = { files: paths, modules: settings.modules, actions: JSON.parse(actionsSource || '{"actions":{}}').actions ?? {} };
if (functions) validateFunctions(functions, functionContext);
if (paths.some((file) => file.startsWith('src/') && /\.(?:ts|tsx|mts|cts)$/.test(file) && !/\.d\.[mc]?ts$/.test(file))) typecheck();
const artifact = await assembleStaticArtifact({
  paths,
  read: async (file) => {
    const bytes = await readFile(path.join(app, file));
    try { return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes); } catch { return new Uint8Array(bytes); }
  },
  compile: async () => {
    const entry = appEntry(paths);
    if (entry === null) throw new Error('Expected ' + appEntries.map((file) => 'app/' + file).join(', ') + '.');
    const { script, styles } = await bundleModule(app, entry, development);
    const sourceStyles = paths.includes('styles.css') ? await readFile(path.join(app, 'styles.css'), 'utf8') : '';
    return { script, styles: [sourceStyles, styles].filter(Boolean).join('\n') };
  },
  bundle: (relative) => bundleModule(app, relative, development),
  runtime,
  sdk,
  routing: settings.manifest?.routing,
  warn: (message) => console.warn('Warning: ' + message),
});
let serverArtifact;
if (functions && Object.keys(functions.functions).length) {
  validateFunctions(functions, { ...functionContext, staticPaths: Object.keys(artifact.files) });
  serverArtifact = await bundleFunctions(app, functions, development);
  validateServerArtifact(functions, serverArtifact);
}
const server = path.join(root, 'server');
await rm(server, { recursive: true, force: true });
if (serverArtifact) {
  await mkdir(server, { recursive: true });
  const hashes = {};
  for (const [name, bundle] of Object.entries(serverArtifact.bundles)) {
    hashes[name] = createHash('sha256').update(bundle).digest('hex');
    await writeFile(path.join(server, name + '.mjs'), bundle);
  }
  await writeFile(path.join(server, 'manifest.json'), JSON.stringify({ version: 1, abi: 'konae-functions/1', manifest: functions, hashes }, null, 2));
}
await rm(dist, { recursive: true, force: true });
for (const [file, contents] of Object.entries(artifact.files)) {
  const target = path.join(dist, file);
  await mkdir(path.dirname(target), { recursive: true });
  await writeFile(target, artifact.encodings?.[file] === 'base64' ? Buffer.from(contents, 'base64') : contents);
}
console.log('Built dist/');

// tsconfig.json includes app/src/ and actions.d.ts, which types miniapp.actions.call for each action.
function typecheck() {
  let tsc;
  try { tsc = createRequire(import.meta.url).resolve('typescript/bin/tsc'); }
  catch { throw new Error('app/ holds TypeScript, and the build checks its types with the typescript package, which is not installed. Run npm install, then build again.'); }
  const checked = spawnSync(process.execPath, [tsc, '--noEmit', '--pretty', 'false', '-p', path.join(root, 'tsconfig.json')], { cwd: root, stdio: 'inherit' });
  if (checked.status !== 0) {
    console.error('The type check failed, so dist/ is unchanged. Fix the errors above. A call to miniapp.actions.call must match an action in actions.d.ts, which konae dev and konae contract write from app/actions.json.');
    process.exit(1);
  }
}

async function walk(prefix) {
  const result = [];
  for (const entry of await readdir(path.join(app, prefix), { withFileTypes: true })) {
    if (entry.name.startsWith('.') || entry.name === 'node_modules') continue;
    const relative = prefix ? prefix + '/' + entry.name : entry.name;
    if (entry.isDirectory()) result.push(...await walk(relative));
    else if (entry.isFile()) result.push(relative);
  }
  return result;
}
