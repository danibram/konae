import deployScript from "./export-scripts/deploy.mjs?raw";
import { createTar, normalizeArchivePath, type TarEntry } from "./tar";
import { normalizeProjectPath } from "./project-files";
import { runtimeScriptAttribute, type BuildArtifact } from "../runtime/artifact";
import { runtimeFile } from "./static-artifact.js";
import staticArtifactSource from "./static-artifact.js?raw";
import type { MiniAppProject } from "./types";
import type { PortableProjectState } from "../deployments/client";
import { checkPackageManifest, type PackageManifest } from "./portable-package";
import { actionContractSchema, generateActionContract, renderActionContractMarkdown, serializeActionContract } from "./action-contract";

export type ExportFile = { path: string; contents: string | Uint8Array };

const maxPortableDatabaseBytes = 16 * 1024 * 1024;
const maxPortableIdentityBytes = 16 * 1024 * 1024;
const maxPortableFilesBytes = 16 * 1024 * 1024;

const dockerfile = `FROM rust:1.91-bookworm AS build

WORKDIR /src
COPY kernel-rs/Cargo.toml kernel-rs/Cargo.lock ./
COPY kernel-rs/src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN groupadd --system --gid 10001 konae \\
  && useradd --system --uid 10001 --gid konae --home-dir /data --no-create-home --shell /usr/sbin/nologin konae \\
  && mkdir -p /data \\
  && chown konae:konae /data \\
  && chmod 0700 /data
COPY --from=build /src/target/release/konae-kernel /usr/local/bin/konae-kernel
COPY entrypoint.sh /usr/local/bin/konae-entrypoint
RUN chmod 755 /usr/local/bin/konae-entrypoint
ENV KONAE_BIND=0.0.0.0 KONAE_DATA_DIR=/data
EXPOSE 8787
ENTRYPOINT ["/usr/local/bin/konae-entrypoint"]
`;

const entrypoint = `#!/bin/sh
set -eu
if [ "$(id -u)" = 0 ]; then
  mkdir -p /data
  chown konae:konae /data
  chmod 0700 /data
  export HOME=/data
  exec setpriv --reuid=konae --regid=konae --init-groups --inh-caps=-all -- /usr/local/bin/konae-kernel
fi
exec /usr/local/bin/konae-kernel
`;

const composeFile = `services:
  kernel:
    build:
      context: .
      dockerfile: Dockerfile
    env_file:
      - .env
    environment:
      KONAE_BIND: "0.0.0.0"
      KONAE_KERNEL_PORT: "8787"
      KONAE_DATA_DIR: /data
      KONAE_PUBLIC_ORIGIN: \u0024{KONAE_PUBLIC_ORIGIN:-http://localhost:\u0024{KONAE_PORT:-8787}}
      KONAE_HOST_SUFFIX: \u0024{KONAE_HOST_SUFFIX:-localhost}
    ports:
      - "127.0.0.1:\u0024{KONAE_PORT:-8787}:8787"
    volumes:
      - ./data:/data
`;

const packageManifest = `{
  "name": "konae-selfhosted-app",
  "private": true,
  "type": "module",
  "scripts": {
    "build": "node scripts/build.mjs",
    "deploy": "node scripts/deploy.mjs",
    "start": "sh start.sh"
  },
  "devDependencies": {
    "esbuild": "0.25.12"
  }
}
`;

// The kit rebuilds with the same static publishing contract as the CLI and
// the editor: scripts/static-artifact.mjs is src/lib/static-artifact.js as is.
const buildScript = `import { readFile, readdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { build } from 'esbuild';
import { assembleStaticArtifact } from './static-artifact.mjs';

const root = process.cwd();
const app = path.join(root, 'app');
const artifactTemplate = JSON.parse(await readFile(path.join(root, 'konae/artifact.json'), 'utf8'));
const project = JSON.parse(await readFile(path.join(root, 'konae/project.json'), 'utf8'));
// The SDK the app was built with: runtime.js when a page loads it, else the marked script in index.html.
const runtime = artifactTemplate.files['runtime.js']
  ?? artifactTemplate.files['index.html']?.match(/<script data-konae-runtime>([\\s\\S]*?)<\\/script>/)?.[1];
if (runtime === undefined) throw new Error('The exported artifact does not contain the Konae runtime.');
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
  routing: project.manifest?.routing,
});
await writeFile(path.join(root, 'konae/artifact.json'), JSON.stringify(artifact, null, 2));
console.log('Built konae/artifact.json');

async function bundle(entry) {
  const result = await build({
    entryPoints: [path.join(app, entry)],
    absWorkingDir: app,
    bundle: true,
    write: false,
    format: 'esm',
    platform: 'browser',
    target: 'es2022',
    outdir: 'out',
  });
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


const startScript = `#!/bin/sh
set -eu

cd "$(dirname "$0")"
if ! command -v docker >/dev/null 2>&1; then
  echo "Docker is required to run this app's local Kernel." >&2
  exit 1
fi
if ! command -v curl >/dev/null 2>&1 || ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
  echo "Docker, Node.js, npm, and curl are required to start the app." >&2
  exit 1
fi
if [ ! -f .env ]; then
  if command -v openssl >/dev/null 2>&1; then
    admin_key=$(openssl rand -hex 32)
  else
    if ! command -v python3 >/dev/null 2>&1; then
      echo "OpenSSL or Python 3 is required to create a local admin key." >&2
      exit 1
    fi
    admin_key=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
  fi
  cat > .env <<EOF
KONAE_PORT=8787
KONAE_ADMIN_KEY=$admin_key
EOF
fi
set -a
. ./.env
set +a
port=\u0024{KONAE_PORT:-8787}
if [ ! -d node_modules ]; then npm install; fi
if [ -f app/package.json ] && [ ! -d app/node_modules ]; then (cd app && npm install); fi
npm run build
docker compose up -d --build
ready=0
for attempt in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  if curl --fail --silent "http://127.0.0.1:\u0024port/_konae/health" >/dev/null; then
    ready=1
    break
  fi
  sleep 1
done
if [ "$ready" -ne 1 ]; then
  echo "The local Kernel did not become ready. Run 'docker compose logs kernel' for details." >&2
  exit 1
fi
npm run deploy
`;

const readme = `# Self-hosted app

This folder contains the app source, its built static files, and a local copy of the Konae Kernel. The Kernel stores the live SQLite database under \u0060data/\u0060 and does not need an object store.

Install Node.js 20 or newer, npm, Docker, and curl before starting.

## Start

Run \u0060./start.sh\u0060. The script installs the pinned build tool, rebuilds the app, starts the Kernel, and loads the app. It creates \u0060.env\u0060 with a local admin key the first time it runs. Open the URL printed by the script. Docker publishes the Kernel only on 127.0.0.1, and the process drops to the unprivileged user \u0060konae\u0060 after locking \u0060data/\u0060 to that user.

Edit the files under \u0060app/\u0060, then run \u0060npm run build\u0060 and \u0060npm run deploy\u0060. When \u0060app/package.json\u0060 lists packages the app imports, \u0060start.sh\u0060 installs them in \u0060app/node_modules\u0060 and the build bundles them into the app; after you change that list, run \u0060npm install\u0060 in \u0060app/\u0060. The Kernel serves the new artifact at the same local app URL.

Run \u0060docker compose down\u0060 to stop the Kernel. The app database and its backups remain under \u0060data/\u0060.

## Where the app lives

Everything the app is sits in one folder, \u0060data/deployments/<id>/\u0060: \u0060deployment.json\u0060, the built \u0060artifact/\u0060, \u0060database.sqlite\u0060, the accounts in \u0060identity.sqlite\u0060, Usage and Signals in \u0060usage.sqlite\u0060, uploads in \u0060files/\u0060 and \u0060backups/\u0060. The owner account is in \u0060data/owners.sqlite\u0060 and the project registry in \u0060data/mcp.sqlite\u0060.

Set \u0060KONAE_COPY_DIR\u0060 to a directory on another disk and the Kernel copies \u0060data/\u0060 there every hour, keeping the last 24 copies and one a day for 30 days. To move to a new machine, start from an empty data directory and run \u0060KONAE_DATA_DIR=data cargo run --release --manifest-path kernel-rs/Cargo.toml -- restore --from <copy directory>\u0060, then start the Kernel. Restore checks every checksum and signs everyone out.

## Run without Docker

Install Rust 1.91 or newer as well. Create \u0060.env\u0060 from \u0060.env.example\u0060 with a long random \u0060KONAE_ADMIN_KEY\u0060, then start the Kernel from this folder:

\u0060\u0060\u0060sh
npm install
if [ -f app/package.json ]; then (cd app && npm install); fi
npm run build
set -a; . ./.env; set +a
KONAE_DATA_DIR=data KONAE_KERNEL_PORT=\u0024{KONAE_PORT:-8787} KONAE_PUBLIC_ORIGIN=\u0024{KONAE_PUBLIC_ORIGIN:-http://localhost:\u0024{KONAE_PORT:-8787}} KONAE_HOST_SUFFIX=\u0024{KONAE_HOST_SUFFIX:-localhost} cargo run --release --locked --manifest-path kernel-rs/Cargo.toml
\u0060\u0060\u0060

When it answers on \u0060/_konae/health\u0060, run \u0060npm run deploy\u0060 in a second terminal. The database lives under \u0060data/\u0060 here too.

## Serve it on your own domain

The kit starts on localhost. To answer on a domain you control, choose three names before the first \u0060npm run deploy\u0060, because the Kernel names the app's generated address when it creates the app:

- the Kernel's own host, such as \u0060konae.example.org\u0060. Owner sign-in answers only there. The deploy script reaches the Kernel on 127.0.0.1 and names this host itself, so the deploy does not need it to be public.
- a suffix for generated app addresses, such as \u0060apps.example.org\u0060.
- the app's hostname, such as \u0060notes.example.org\u0060. It cannot be the Kernel's host or sit under the suffix.

Add them to \u0060.env\u0060 next to a long random \u0060KONAE_ADMIN_KEY\u0060:

\u0060\u0060\u0060sh
KONAE_PUBLIC_ORIGIN=https://konae.example.org
KONAE_HOST_SUFFIX=apps.example.org
KONAE_APP_DOMAIN=notes.example.org
\u0060\u0060\u0060

Point the app's hostname at this server. Put a proxy such as nginx or Caddy in front of the Kernel: it terminates TLS for the app's hostname and forwards to \u0060127.0.0.1:$KONAE_PORT\u0060 with the Host header unchanged. The Kernel issues no certificate. Then run \u0060./start.sh\u0060, or \u0060npm run deploy\u0060 if the Kernel is already running.

The first run prints a TXT record, \u0060_konae.notes.example.org\u0060 and a token. Add it at your DNS provider and run \u0060npm run deploy\u0060 again. Once the Kernel finds the token, the script prints the address the app answers at. A verified name stays verified until you clear it, and the generated address keeps working.

An export carries the app's custom hostname without its verification token, so a kit made from an app that already had one issues a new token on this Kernel. Without \u0060KONAE_APP_DOMAIN\u0060 the script prints the record and asks you to set it.

On the domain the app keeps its accounts, rows and rules. Sessions do not cross between the generated address and the app's hostname, so a person signs in on each. Mail links use the app's hostname once it is verified.

## Changing the database

\u0060app/schema.ts\u0060 is the Database Model the Konae editor used to plan migrations. This export has no migration generator, so editing it changes nothing. Change \u0060app/schema.sql\u0060 instead, and only in ways the Kernel accepts on every deploy: new tables and indexes with \u0060IF NOT EXISTS\u0060. Back up \u0060data/\u0060 before any change to an existing table.

Keep \u0060.env\u0060 private. It contains the owner key for this local Kernel. The exported app source and static artifact contain no Konae-hosted credentials.

## The Action Contract

\u0060konae/action-contract.json\u0060 lists every Server Action in \u0060app/actions.json\u0060: its inputs, the columns or counts it returns, who may call it, its HTTP route, the errors the Kernel returns for it and the limits that apply. \u0060konae/action-contract.schema.json\u0060 is its JSON Schema and \u0060konae/action-contract.md\u0060 the same content to read. A backend that replaces this Kernel can check itself against it with \u0060bun run konae -- conform\u0060 from the Konae repository. The files describe the app as exported. After editing \u0060app/actions.json\u0060 or \u0060app/schema.sql\u0060, write them again from the Konae repository with \u0060bun run konae -- contract --dir <this folder>/app --out <this folder>/konae\u0060. Keep them out of \u0060app/\u0060: \u0060npm run deploy\u0060 uploads every file there as app source.

## App data

An export made with \u0060--with-data\u0060 holds the app's data in \u0060konae/package/\u0060. \u0060manifest.json\u0060 lists the database, the accounts, the files and, when the app recorded any, its Usage and Signals, with the size and SHA-256 of each, and names the published revision they came from. The first \u0060npm run deploy\u0060 imports them. It uploads each part in 64 KiB blocks and records its progress in \u0060konae/import-state.json\u0060, so running it again after an interruption continues where it stopped. The Kernel checks every checksum, the accounts each file names and the free space before it creates the app, and refuses a package for an app that already exists. If the script stops after the Kernel created the app, running it again returns the same receipt. Later runs publish the code and keep the data.

Accounts keep their IDs, password hashes, verification state and suspensions, and the app keeps its visibility and signup policy. Sessions and open email links stay behind, so everyone signs in again. API keys arrive revoked, and \u0060api_keys_list\u0060 names each one with what it could call and \u0060revokedReason: "import"\u0060, so you can issue new keys for the programs that still need them. The package holds personal data and password hashes. Keep this folder private and out of Git, and delete \u0060konae/package/\u0060 once the app runs.

Exports made before packages hold \u0060konae/initial-database.sqlite\u0060, \u0060konae/initial-identity.sqlite\u0060, \u0060konae/initial-files.portable\u0060 and \u0060konae/initial-signup-policy.json\u0060 instead, at most 16 MiB together. The deploy script still imports them the same way.
`;

export function buildProjectExport(
  project: MiniAppProject,
  artifact: BuildArtifact,
  kernelFiles: readonly ExportFile[],
  state?: Uint8Array | PortableProjectState,
): { archive: Uint8Array; entries: TarEntry[] } {
  const entries = projectExportEntries(project, artifact, kernelFiles, state);
  return { archive: createTar(entries), entries };
}

/**
 * The kit's entries in archive order. With a package manifest the kit names
 * the package's visibility and carries `konae/package/manifest.json`; the
 * caller appends the component files from disk, so they never sit in memory.
 */
export function projectExportEntries(
  project: MiniAppProject,
  artifact: BuildArtifact,
  kernelFiles: readonly ExportFile[],
  state?: Uint8Array | PortableProjectState | { package: PackageManifest },
): TarEntry[] {
  if (state && "package" in state) {
    const manifest = checkPackageManifest(state.package);
    const entries = projectExportEntries(project, artifact, kernelFiles, { visibility: manifest.visibility, domain: manifest.domain } as PortableProjectState);
    entries.splice(entries.findIndex((entry) => entry.path === "konae/artifact.json") + 1, 0,
      { path: "konae/package/manifest.json", contents: JSON.stringify(manifest, null, 2), mode: 0o600 });
    return entries;
  }
  if (artifact.version !== 1 || artifact.entrypoint !== "index.html" || !artifact.files["index.html"]) {
    throw new Error("The export requires a valid version 1 Build Artifact.");
  }
  // scripts/build.mjs finds the runtime by this marker; without it every rebuild would fail.
  if (!artifact.files["index.html"].includes(`<script ${runtimeScriptAttribute}>`) && artifact.files[runtimeFile] === undefined) {
    throw new Error("The Build Artifact does not mark its Konae runtime script. Rebuild the project before exporting.");
  }
  const portableState = state instanceof Uint8Array ? { initialDatabase: state } : state;
  if (portableState?.visibility !== undefined && portableState.visibility !== "public" && portableState.visibility !== "private") {
    throw new Error("The portable snapshot has invalid visibility.");
  }
  validatePortableBytes("SQLite snapshot", portableState?.initialDatabase, maxPortableDatabaseBytes);
  validatePortableBytes("Auth database", portableState?.initialIdentity, maxPortableIdentityBytes);
  validatePortableBytes("File data", portableState?.initialFiles, maxPortableFilesBytes);
  const portableBytes = (portableState?.initialDatabase?.byteLength ?? 0)
    + (portableState?.initialIdentity?.byteLength ?? 0)
    + (portableState?.initialFiles?.byteLength ?? 0);
  if (portableBytes > maxPortableDatabaseBytes) {
    throw new Error("Combined portable state exceeds the 16 MiB self-host import limit.");
  }
  const files = Object.entries(project.files).map(([path, contents]) => ({
    path: `app/${safeProjectPath(path)}`,
    contents,
  }));
  if (!project.files["schema.ts"] && project.databaseModel.source) {
    files.push({ path: "app/schema.ts", contents: project.databaseModel.source });
  }
  const contract = actionContractEntries(project.files["actions.json"], project.files["schema.sql"] ?? "", { id: project.id, name: project.name });
  const normalizedArtifact = {
    version: artifact.version,
    entrypoint: artifact.entrypoint,
    files: artifact.files,
    ...(artifact.encodings ? { encodings: artifact.encodings } : {}),
  };
  const entries: TarEntry[] = [
    { path: "README.md", contents: readme },
    { path: "Dockerfile", contents: dockerfile },
    { path: "entrypoint.sh", contents: entrypoint, mode: 0o755 },
    { path: "compose.yaml", contents: composeFile },
    { path: ".env.example", contents: "KONAE_PORT=8787\nKONAE_ADMIN_KEY=replace-me\n# To serve on your own domain, see \"Serve it on your own domain\" in README.md.\n# KONAE_PUBLIC_ORIGIN=https://konae.example.org\n# KONAE_HOST_SUFFIX=apps.example.org\n# KONAE_APP_DOMAIN=notes.example.org\n" },
    { path: ".gitignore", contents: ".env\ndata/\nnode_modules/\nkonae/initial-*\nkonae/package/\nkonae/import-state.json\n" },
    { path: "package.json", contents: packageManifest },
    { path: "scripts/build.mjs", contents: buildScript },
    { path: "scripts/static-artifact.mjs", contents: staticArtifactSource },
    { path: "scripts/deploy.mjs", contents: deployScript },
    { path: "start.sh", contents: startScript, mode: 0o755 },
    { path: "konae/project.json", contents: JSON.stringify({
      id: project.id,
      name: project.name,
      slug: project.slug,
      manifest: portableState?.visibility ? { ...project.manifest, visibility: portableState.visibility } : project.manifest,
      databaseModel: project.databaseModel,
      ...(portableState?.domain ? { domain: portableState.domain } : {}),
    }, null, 2) },
    { path: "konae/artifact.json", contents: JSON.stringify(normalizedArtifact, null, 2) },
    ...contract,
    ...(portableState?.initialDatabase ? [{ path: "konae/initial-database.sqlite", contents: portableState.initialDatabase, mode: 0o600 }] : []),
    ...(portableState?.initialIdentity ? [{ path: "konae/initial-identity.sqlite", contents: portableState.initialIdentity, mode: 0o600 }] : []),
    ...(portableState?.initialFiles ? [{ path: "konae/initial-files.portable", contents: portableState.initialFiles, mode: 0o600 }] : []),
    ...(portableState?.initialSignupPolicy ? [{ path: "konae/initial-signup-policy.json", contents: JSON.stringify(portableState.initialSignupPolicy), mode: 0o600 }] : []),
    ...files,
    ...kernelFiles.map((file) => ({ path: safeProjectPath(file.path), contents: file.contents })),
  ];
  const uniquePaths = new Set<string>();
  for (const entry of entries) {
    const path = normalizeArchivePath(entry.path);
    if (uniquePaths.has(path)) throw new Error(`Duplicate export path: ${path}`);
    uniquePaths.add(path);
  }
  return entries;
}

/** The Action Contract of the exported actions.json, or nothing for a Capsule without one. */
function actionContractEntries(actionsSource: string | undefined, schemaSource: string, project: { id: string; name: string }): TarEntry[] {
  if (actionsSource === undefined) return [];
  let contract;
  try {
    contract = generateActionContract({ actionsSource, schemaSource, project }).contract;
  } catch (error) {
    throw new Error(`The export cannot describe its Server Actions: ${error instanceof Error ? error.message : String(error)}`);
  }
  return [
    { path: "konae/action-contract.json", contents: serializeActionContract(contract) },
    { path: "konae/action-contract.md", contents: renderActionContractMarkdown(contract) },
    { path: "konae/action-contract.schema.json", contents: `${JSON.stringify(actionContractSchema, null, 2)}\n` },
  ];
}

function validatePortableBytes(label: string, bytes: Uint8Array | undefined, maxBytes: number) {
  if (bytes && bytes.byteLength > maxBytes) {
    throw new Error(`${label} exceeds the ${Math.floor(maxBytes / (1024 * 1024))} MiB self-host import limit.`);
  }
}

function safeProjectPath(path: string) {
  const normalized = path.replaceAll("\\", "/");
  if (normalizeProjectPath(normalized) !== normalized) {
    throw new Error(`Invalid project file path: ${path}`);
  }
  return normalizeArchivePath(normalized);
}

