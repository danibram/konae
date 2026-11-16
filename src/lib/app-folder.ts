











import { generateActionContract, renderActionContractMarkdown } from "./action-contract";
import { ACTION_TYPES_FILE, renderActionTypes } from "./action-types";
import type { PackageComponentName } from "./portable-package";
import { readTar } from "./tar";
import { version } from "../../package.json";
import { sdkContract } from "../sdk/contract";

export type FolderEntry = { path: string; contents: string | Uint8Array };

export type SignupPolicy = { mode: "open" | "invitation" | "closed" | "allowlist"; allowlist?: string[] };


export type AppSettings = {
  id: string;
  name: string;
  slug: string;
  visibility: "public" | "private";
  signupPolicy?: SignupPolicy;
  signInMethods?: ("password" | "google")[];
  contactEmail?: string;
  modules?: Record<string, { on: boolean; [option: string]: unknown }>;

  kernel: string;

  manifest: Record<string, unknown>;
};

export const SETTINGS_FILE = "konae.json";
export const SOURCES_FOLDER = "app";
export const DIST_FOLDER = "dist";
export const DATA_FOLDER = "data";

export const KERNEL_VERSION: string = version;

export function defaultModuleStates(existing: AppSettings["modules"] = {}): AppSettings["modules"] {
  return Object.fromEntries(sdkContract.modules.filter((module) => module.switch).map((module) => [module.name, Object.assign(
    { on: false },
    Object.fromEntries(module.options.map((option) => [option.key, option.default])),
    existing?.[module.name] ?? {},
  )]));
}

export function settingsForFolder(settings: AppSettings): AppSettings {
  const modules = defaultModuleStates(settings.modules);
  const auth = modules?.auth;
  if (auth) {
    settings = {
      ...settings,
      signupPolicy: settings.signupPolicy ?? auth.signupPolicy as SignupPolicy | undefined ?? { mode: settings.visibility === "public" ? "open" : "closed", allowlist: [] },
      signInMethods: settings.signInMethods ?? auth.signInMethods as AppSettings["signInMethods"] ?? ["password"],
    };
    modules!.auth = { on: auth.on };
  }
  return { ...settings, modules };
}


export const dataPaths: Record<PackageComponentName, string> = {
  database: "data/database.sqlite",
  identity: "data/identity.sqlite",
  files: "data/files",
  usage: "data/usage.sqlite",
};


export function dataEntries(name: PackageComponentName, bytes: Uint8Array): FolderEntry[] {
  if (name !== "files") return [{ path: dataPaths[name], contents: bytes }];
  return readTar(bytes).map((entry) => ({ path: `${dataPaths.files}/${entry.path}`, contents: entry.contents }));
}

/** The secrets the app's routes and calls sign with, by name. */
export function secretNames(actionsSource: string): string[] {
  let actions: Record<string, { http?: { signature?: { secret?: unknown } }; auth?: { secret?: unknown } }> = {};
  try { actions = JSON.parse(actionsSource).actions ?? {}; } catch { return []; }
  const names = new Set<string>();
  for (const action of Object.values(actions)) {
    if (typeof action?.http?.signature?.secret === "string") names.add(action.http.signature.secret);
    if (typeof action?.auth?.secret === "string") names.add(action.auth.secret);
  }
  return [...names].sort();
}

/**
 * The files the folder carries beside `app/`, `dist/` and `data/`. `runtime`
 * is the SDK (src/mcp/runtime.js) and `staticArtifact` the build rules
 * (src/lib/static-artifact.js), and `localBuilder` holds the esbuild call
 * both local build doors run.
 */
export function generatedFiles(input: { settings: AppSettings; actionsSource: string; schemaSource: string; runtime: string; staticArtifact: string; localBuilder: string }): FolderEntry[] {
  const { settings } = input;
  const { contract } = generateActionContract({ actionsSource: input.actionsSource, schemaSource: input.schemaSource, project: { id: settings.id, name: settings.name } });
  return [
    { path: "compose.yaml", contents: composeFile(settings) },
    { path: ".env.example", contents: envExample(settings) },
    { path: ".gitignore", contents: gitignore },
    { path: "AGENTS.md", contents: agentsGuide(settings, secretNames(input.actionsSource)) },
    { path: "action-contract.md", contents: renderActionContractMarkdown(contract) },
    { path: ACTION_TYPES_FILE, contents: renderActionTypes(contract) },
    { path: "package.json", contents: packageManifest(settings) },
    { path: "tsconfig.json", contents: tsconfig },
    { path: "scripts/build.mjs", contents: buildScript },
    { path: "scripts/static-artifact.mjs", contents: input.staticArtifact },
    { path: "scripts/local-builder.mjs", contents: input.localBuilder },
    { path: "scripts/runtime.js", contents: input.runtime },
  ];
}

export const gitignore = `data/
.env
.konae/
node_modules/
`;

/** The image is the Kernel version konae.json names, so an exported folder runs the Kernel that wrote it. */
function envExample(settings: AppSettings) {
  return `# The Kernel's settings for compose.yaml. Copy to .env, which stays on this machine.
# The image to run. A release publishes this tag for linux/amd64 and linux/arm64. A local image can replace it.
KONAE_KERNEL_IMAGE=ghcr.io/danibram/konae-kernel:${settings.kernel}
KONAE_PORT=8787
KONAE_PUBLIC_ORIGIN=http://localhost:8787
KONAE_HOST_SUFFIX=localhost
# The Kernel seals the links in queued account email with a key it writes to mail-key in its volume
# on the first start. Set KONAE_MAIL_SECRET, at least 32 characters, to use your own instead.
# How account email leaves: resend, smtp or development, which writes it to mailbox.jsonl in the Kernel's volume.
# Without a transport, invitations and password resets are refused.
KONAE_MAIL_TRANSPORT=
`;
}

function composeFile(settings: AppSettings) {
  return `# Runs the Kernel on this folder. The Kernel opens it as apps/${settings.id}
# of its data directory, checks it as a release, and serves it. Set
# KONAE_KERNEL_IMAGE in .env to a published or local Kernel image.
services:
  kernel:
    image: \${KONAE_KERNEL_IMAGE:?Set KONAE_KERNEL_IMAGE in .env to the Kernel image.}
    env_file: .env
    environment:
      KONAE_BIND: "0.0.0.0"
      KONAE_KERNEL_PORT: "8787"
      KONAE_DATA_DIR: /kernel
    ports:
      - "127.0.0.1:\${KONAE_PORT:-8787}:8787"
    volumes:
      - ./:/kernel/apps/${settings.id}
      - kernel:/kernel
volumes:
  kernel:
`;
}

function packageManifest(settings: AppSettings) {
  return `${JSON.stringify({
    name: settings.slug || "konae-app",
    private: true,
    type: "module",
    scripts: { build: "node scripts/build.mjs" },
    devDependencies: { esbuild: "0.25.12", typescript: "5.9.3" },
  }, null, 2)}\n`;
}

/**
 * What `npm run build` type-checks a TypeScript app against: app/src/, the
 * files it imports, and actions.d.ts. A script beside the app, such as a
 * replay script at the root of app/, stays out. Not strict, so a type that is only implicit passes; a call
 * to an action that does not exist, or with an input it does not take, fails.
 */
const tsconfig = `${JSON.stringify({
  compilerOptions: {
    target: "ES2022",
    lib: ["ES2022", "DOM", "DOM.Iterable"],
    module: "ESNext",
    moduleResolution: "Bundler",
    jsx: "react-jsx",
    allowImportingTsExtensions: true,
    resolveJsonModule: true,
    isolatedModules: true,
    skipLibCheck: true,
    noEmit: true,
  },
  include: ["app/src", ACTION_TYPES_FILE],
}, null, 2)}\n`;

function agentsGuide(settings: AppSettings, secrets: string[]) {
  return `# ${settings.name}

This folder is one Konae app. The Kernel serves the pages in \`dist/\` and runs the Server Actions in \`app/actions.json\` on the database in \`data/\`.

## Layout

- \`konae.json\`: the app's name, visibility (${settings.visibility}), module state, contact email and the Kernel version.
- \`app/\`: the editable sources. It may also hold binary assets. Konae checks the same path, count and decoded-size limits before saving and publishing them.
- \`dist/\`: the built files. \`npm run build\` writes it from \`app/\`. Edit \`app/\`, never \`dist/\`.
- \`data/\`: \`database.sqlite\`, the app accounts in \`identity.sqlite\`, Usage in \`usage.sqlite\` and uploads in \`files/\`. It holds people's accounts, so it never goes into Git.
- \`action-contract.md\`: every Server Action with its inputs and the columns it returns.
- \`actions.d.ts\`: the types of \`miniapp.actions.call\` for each action a page may call. When \`app/src/\` holds TypeScript, \`npm run build\` checks it, and what it imports, against them with \`tsconfig.json\` and stops on an error.

## Rules the Kernel checks

An optional \`schema-transforms.json\` records ordered data changes between schema fingerprints. The Kernel stores applied history in \`data/database.sqlite\`; the definition travels with app sources and never ships in \`dist/\`. The Validation Gate checks its shape, order, bounds and SQL subset. It cannot prove a transition against a real database.

- The page talks to the database only through the named Server Actions in \`app/actions.json\`. \`action-contract.md\` lists them.
- A query that reads rows names a LIMIT.
- \`schema.sql\` is the model. Add tables and columns there; the Kernel applies an added table, column or index on the next publish or start, and a change that drops or retypes one only with the confirmation it names.
- A page loads no script from another origin, uses no inline event handler, and runs no WebAssembly or \`eval\`. It fetches only from its own origin and the origins \`manifest.connects\` in \`konae.json\` names.

## Module settings

| Need | Use | Not |
| --- | --- | --- |
| Turn an app capability on or set its options | \`modules_read\` and \`module_set\`, then read it back with \`modules_read\` | A separate tool or a setting in the Capsule |
| Be listed in search | Turn on \`seo\` with \`module_set\`, after \`modules_read\` | Assume a public app is listed |

## Rules nothing checks

- Text from a row, an upload or a user is written into the page with \`textContent\` or the template's \`escapeHtml\`, in attributes too, and is never concatenated into HTML unescaped.

## Run it

1. To write \`dist/\`, if \`package.json\` lists an npm package, run \`npm install\` once, then \`npm run build\`. The shared local builder builds a template without an install. \`dev\` writes it without \`npm install\` when no imported package needs one. If \`package.json\` has a build script but \`node_modules/\` is absent, it uses the built-in builder. Once that directory exists, the builder runs \`npm run build\`. An exported folder already carries its built \`dist/\`.
2. Run \`konae dev --dir <this folder>\`. It serves the app with \`data/\` and starts again when a file in \`app/\` changes. From a repository checkout, it uses that checkout's Kernel and builds it with Cargo if absent. The compiled CLI downloads the matching Kernel binary and verifies its checksum unless \`KONAE_KERNEL_BIN\` names a local one.
3. On a server: copy \`.env.example\` to \`.env\`, fill it in, and run \`docker compose up -d\`. The Kernel prints the app's address, \`http://${settings.slug || "app"}.localhost:8787\` with the example settings.
4. To publish on a Konae installation: \`konae login --browser --dir <this folder> --url <installation>\`, which signs the owner in through the browser, then \`konae publish --dir <this folder>\`. \`konae push --dir <this folder>\` prints what replacing the published app's data with \`data/\` would change, and with \`--yes\` it does so after a backup. A pending pull blocks push until you import the pulled data with \`dev --reset\`.

At startup the Kernel checks the folder as Konae checks a release. It refuses a folder whose \`dist/\` breaks the publishing rules, whose actions break a rule above, or whose \`schema.sql\` drops or retypes a column, and says which. To apply a change that loses data, start it once with the \`KONAE_CONFIRM_SCHEMA\` value it names.
${secrets.length ? `\nThe app's routes and calls sign with ${secrets.length === 1 ? "the secret" : "the secrets"} ${secrets.join(", ")}. A secret never travels with the folder, so set each where the app runs: \`kernel secret set <name>\` below, or \`bun run konae -- secret set <name> --dir <this folder>\` on Konae, which reads the value from standard input. The Kernel does not serve the app until each is set.\n` : ""}
## Commands

Outside Konae, the Kernel's commands manage the app. Each refuses with the same words as the tool in Konae's editor, and prints the answer as JSON. Under Compose, run each as \`docker compose exec kernel kernel <command>\`. All but \`import\` work while the Kernel serves.

- \`kernel users [--state pending|active|suspended]\`: the app accounts, with their IDs.
- \`kernel invite <email>...\`: email each person an invitation. It needs a mail transport.
- \`kernel suspend <user id>\`, \`kernel resume <user id>\`: end an account's sessions and keep it out, or let it back in.
- \`kernel signup-policy open|invitation|closed|allowlist [<email or @domain>...]\`: who may create an account.
- \`kernel api-key create <name> --action <name>... [--files public|private]...\`, \`kernel api-key list\`, \`kernel api-key revoke <key id>\`: keys for the app's HTTP routes and files. The key appears once.
- \`kernel secret set <name> [--use route|call|google]\`, with the value on standard input, and \`kernel secret list\`.
- \`kernel sql "<statement>"\`: run SQL on \`data/database.sqlite\`.
- \`kernel events --from <YYYY-MM-DD> --to <YYYY-MM-DD>\`: notices, calls, scheduled runs, routes and Signals.
- \`kernel maintenance on|off\`: pause the app, so it takes no writes, signups or sign-ins and holds its email, calls and schedule, or let it run again.
- \`kernel enter <email>\`: a one-time link, good for 2 minutes, that signs a browser in as that account. Usage and Signals do not count what the session does. With no audit log outside Konae, the Kernel's log records who entered as whom, as \`operator:<login name> entered app <id> as <email>.\`
- \`kernel import <folder> [--confirm-schema <confirmation>]\`: replace the app's data with the \`data/\` of an exported folder, after a backup in \`.konae/backups\`. Sessions and API keys stay as they are. It needs the Kernel stopped, so under Compose run \`docker compose stop kernel\`, then \`docker compose run --rm kernel kernel import <folder inside the volume>\`, then \`docker compose start kernel\`.

Add \`--app ${settings.id}\` when the Kernel serves more than one app.
`;
}





const buildScript = `import { spawnSync } from 'node:child_process';
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { appEntries, appEntry, assembleStaticArtifact } from './static-artifact.mjs';
import { bundleModule } from './local-builder.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const app = path.join(root, 'app');
const dist = path.join(root, 'dist');
const development = process.argv.includes('--dev');
const settings = JSON.parse(await readFile(path.join(root, 'konae.json'), 'utf8'));
const runtime = await readFile(path.join(root, 'scripts/runtime.js'), 'utf8');
const paths = await walk('');
if (paths.some((file) => file.startsWith('src/') && /\\.(?:ts|tsx|mts|cts)$/.test(file) && !/\\.d\\.[mc]?ts$/.test(file))) typecheck();
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
    return { script, styles: [sourceStyles, styles].filter(Boolean).join('\\n') };
  },
  bundle: (relative) => bundleModule(app, relative, development),
  runtime,
  routing: settings.manifest?.routing,
  warn: (message) => console.warn('Warning: ' + message),
});
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
`;
