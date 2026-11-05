// The app folder of ADR-0027: what `konae init` writes, what `konae export`
// and the editor's Export hand over, and what the Kernel keeps for each app.
//
//   konae.json      name, visibility, Signup Policy, sign-in methods, contact email, Kernel version
//   app/            the sources: pages, actions.json, schema.sql
//   dist/           the built static files the Kernel serves
//   data/           database.sqlite, identity.sqlite, usage.sqlite, files/
//   compose.yaml, .env.example, AGENTS.md, action-contract.md, actions.d.ts, package.json, scripts/
//
// The CLI writes these entries to disk and the editor packs them in a tar.
import { generateActionContract, renderActionContractMarkdown } from "./action-contract";
import { ACTION_TYPES_FILE, renderActionTypes } from "./action-types";
import type { PackageComponentName } from "./portable-package";
import { readTar } from "./tar";

export type FolderEntry = { path: string; contents: string | Uint8Array };

export type SignupPolicy = { mode: "open" | "invitation" | "closed" | "allowlist"; allowlist?: string[] };

/** konae.json, as kernel-rs/kernel/src/lib.rs `Settings` writes and reads it. */
export type AppSettings = {
  id: string;
  name: string;
  slug: string;
  visibility: "public" | "private";
  signupPolicy: SignupPolicy;
  signInMethods: ("password" | "google")[];
  contactEmail?: string;
  /** The version of the Kernel that wrote the file. */
  kernel: string;
  /** The Capsule manifest without its visibility: routing, connects and the declared actions. */
  manifest: Record<string, unknown>;
};

export const SETTINGS_FILE = "konae.json";
export const SOURCES_FOLDER = "app";
export const DIST_FOLDER = "dist";
export const DATA_FOLDER = "data";
/** The Kernel this checkout builds. kernel-rs/Cargo.toml names the same version. */
export const KERNEL_VERSION = "0.1.0";

/** Where each part of an export's data lands in the folder. */
export const dataPaths: Record<PackageComponentName, string> = {
  database: "data/database.sqlite",
  identity: "data/identity.sqlite",
  files: "data/files",
  usage: "data/usage.sqlite",
};

/** The entries one data component becomes. The files component is a tar of `index.json` and `<id>.blob`. */
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
 * (src/lib/static-artifact.js), so `npm run build` makes the same `dist/` the
 * editor and `konae build` make.
 */
export function generatedFiles(input: { settings: AppSettings; actionsSource: string; schemaSource: string; runtime: string; staticArtifact: string }): FolderEntry[] {
  const { settings } = input;
  const { contract } = generateActionContract({ actionsSource: input.actionsSource, schemaSource: input.schemaSource, project: { id: settings.id, name: settings.name } });
  return [
    { path: "compose.yaml", contents: composeFile(settings) },
    { path: ".env.example", contents: envExample },
    { path: ".gitignore", contents: gitignore },
    { path: "AGENTS.md", contents: agentsGuide(settings, secretNames(input.actionsSource)) },
    { path: "action-contract.md", contents: renderActionContractMarkdown(contract) },
    { path: ACTION_TYPES_FILE, contents: renderActionTypes(contract) },
    { path: "package.json", contents: packageManifest(settings) },
    { path: "scripts/build.mjs", contents: buildScript },
    { path: "scripts/static-artifact.mjs", contents: input.staticArtifact },
    { path: "scripts/runtime.js", contents: input.runtime },
  ];
}

export const gitignore = `data/
.env
.konae/
node_modules/
`;

const envExample = `# The Kernel's settings for compose.yaml. Copy to .env, which stays on this machine.
# The image to run. Set it to where you published the image Dockerfile.kernel builds.
KONAE_KERNEL_IMAGE=
KONAE_PORT=8787
KONAE_PUBLIC_ORIGIN=http://localhost:8787
KONAE_HOST_SUFFIX=localhost
# A long random key that seals the links in queued account email, such as: openssl rand -hex 32
KONAE_ADMIN_KEY=
# How account email leaves: resend, smtp or development, which writes it to mailbox.jsonl in the Kernel's volume.
# Without a transport, invitations and password resets are refused.
KONAE_MAIL_TRANSPORT=
`;

function composeFile(settings: AppSettings) {
  return `# Runs the Kernel on this folder. The Kernel opens it as apps/${settings.id}
# of its data directory, checks it as a release, and serves it. Set
# KONAE_KERNEL_IMAGE in .env to the image Dockerfile.kernel builds.
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
    devDependencies: { esbuild: "0.25.12" },
  }, null, 2)}\n`;
}

function agentsGuide(settings: AppSettings, secrets: string[]) {
  return `# ${settings.name}

This folder is one Konae app. A Konae Kernel serves the pages in \`dist/\` and runs the Server Actions in \`app/actions.json\` on the database in \`data/\`.

## Layout

- \`konae.json\`: the app's name, visibility (${settings.visibility}), Signup Policy, sign-in methods, contact email and the Kernel version.
- \`app/\`: the sources. \`index.html\`, \`app.js\` or \`src/main.ts\`, \`styles.css\`, \`actions.json\` and \`schema.sql\`.
- \`dist/\`: the built files. \`npm run build\` writes it from \`app/\`. Edit \`app/\`, never \`dist/\`.
- \`data/\`: \`database.sqlite\`, the app accounts in \`identity.sqlite\`, Usage in \`usage.sqlite\` and uploads in \`files/\`. It holds people's accounts, so it never goes into Git.
- \`action-contract.md\`: every Server Action with its inputs and the columns it returns.
- \`actions.d.ts\`: the types of \`miniapp.actions.call\` for each action a page may call.

## Rules the Kernel checks

- The page talks to the database only through the named Server Actions in \`app/actions.json\`. \`action-contract.md\` lists them.
- A query that reads rows names a LIMIT.
- \`schema.sql\` is the model. Add tables and columns there; the Kernel applies an added table or column on the next publish and refuses a change that drops or retypes one.
- A page loads no script from another origin and uses no inline event handler.

## Run it

1. \`npm install\`, then \`npm run build\` to write \`dist/\`.
2. On this machine, from a Konae checkout: \`bun run konae -- dev --dir <this folder>\`. It serves the app with \`data/\` and starts again when a file in \`app/\` changes.
3. On a server: copy \`.env.example\` to \`.env\`, fill it in, and run \`docker compose up -d\`. The Kernel prints the app's address, \`http://${settings.slug || "app"}.localhost:8787\` with the example settings.
4. To publish on a Konae installation: \`bun run konae -- login --dir <this folder> --url <installation>\`, then \`bun run konae -- publish --dir <this folder>\`.

At startup the Kernel checks the folder as Konae checks a release. It refuses a folder whose \`dist/\` breaks the publishing rules, whose actions break a rule above, or whose \`schema.sql\` drops or retypes a column, and says which. To apply a change that loses data, start it once with the \`KONAE_CONFIRM_SCHEMA\` value it names.
${secrets.length ? `\nThe app's routes and calls sign with ${secrets.length === 1 ? "the secret" : "the secrets"} ${secrets.join(", ")}. A secret never travels with the folder, so set each where the app runs: \`kernel secret set <name>\` below, or \`secret_set\` on Konae. The Kernel does not serve the app until each is set.\n` : ""}
## Commands

Outside Konae, the Kernel's commands manage the app. Each refuses with the same words as the tool in Konae's editor, and prints the answer as JSON. Under Compose, run each as \`docker compose exec kernel kernel <command>\`; they work while the Kernel serves.

- \`kernel users [--state pending|active|suspended]\`: the app accounts, with their IDs.
- \`kernel invite <email>...\`: email each person an invitation. It needs a mail transport.
- \`kernel suspend <user id>\`, \`kernel resume <user id>\`: end an account's sessions and keep it out, or let it back in.
- \`kernel signup-policy open|invitation|closed|allowlist [<email or @domain>...]\`: who may create an account.
- \`kernel api-key create <name> --action <name>... [--files public|private]...\`, \`kernel api-key list\`, \`kernel api-key revoke <key id>\`: keys for the app's HTTP routes and files. The key appears once.
- \`kernel secret set <name> [--use route|call|google]\`, with the value on standard input, and \`kernel secret list\`.
- \`kernel sql "<statement>"\`: run SQL on \`data/database.sqlite\`.
- \`kernel events --from <YYYY-MM-DD> --to <YYYY-MM-DD>\`: notices, calls, scheduled runs, routes and Signals.

Add \`--app ${settings.id}\` when the Kernel serves more than one app.
`;
}

// Builds app/ into dist/ by the rules in static-artifact.mjs, which the
// editor and `konae build` run too.
const buildScript = `import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { assembleStaticArtifact } from './static-artifact.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const app = path.join(root, 'app');
const dist = path.join(root, 'dist');
const settings = JSON.parse(await readFile(path.join(root, 'konae.json'), 'utf8'));
const runtime = await readFile(path.join(root, 'scripts/runtime.js'), 'utf8');
const paths = await walk('');
const artifact = await assembleStaticArtifact({
  paths,
  read: async (file) => {
    const bytes = await readFile(path.join(app, file));
    try { return new TextDecoder('utf-8', { fatal: true }).decode(bytes); } catch { return new Uint8Array(bytes); }
  },
  compile: async () => {
    const entry = paths.includes('src/main.ts') ? 'src/main.ts' : 'app.js';
    if (!paths.includes(entry)) throw new Error('Expected app/src/main.ts or app/app.js.');
    const { script, styles } = await bundle(entry);
    const sourceStyles = paths.includes('styles.css') ? await readFile(path.join(app, 'styles.css'), 'utf8') : '';
    return { script, styles: [sourceStyles, styles].filter(Boolean).join('\\n') };
  },
  bundle,
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

async function bundle(entry) {
  const result = await build({ entryPoints: [path.join(app, entry)], absWorkingDir: app, bundle: true, write: false, format: 'esm', platform: 'browser', target: 'es2022', outdir: 'out' });
  const script = result.outputFiles.find((file) => file.path.endsWith('.js'))?.text;
  if (script === undefined) throw new Error('The app build did not produce JavaScript.');
  return { script, styles: result.outputFiles.find((file) => file.path.endsWith('.css'))?.text ?? '' };
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
