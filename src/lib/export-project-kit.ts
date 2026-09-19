import deployScript from "./export-scripts/deploy.mjs?raw";
import { createTar, normalizeArchivePath, type TarEntry } from "./tar";
import { normalizeProjectPath } from "./project-files";
import { runtimeScriptAttribute, type BuildArtifact } from "../runtime/artifact";
import type { MiniAppProject } from "./types";
import type { PortableProjectState } from "../deployments/client";
import { checkPackageManifest, type PackageManifest } from "./portable-package";

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
RUN groupadd --system --gid 10001 littlebox \\
  && useradd --system --uid 10001 --gid littlebox --home-dir /data --no-create-home --shell /usr/sbin/nologin littlebox \\
  && mkdir -p /data \\
  && chown littlebox:littlebox /data \\
  && chmod 0700 /data
COPY --from=build /src/target/release/littlebox-kernel /usr/local/bin/littlebox-kernel
COPY entrypoint.sh /usr/local/bin/littlebox-entrypoint
RUN chmod 755 /usr/local/bin/littlebox-entrypoint
ENV LITTLEBOX_BIND=0.0.0.0 LITTLEBOX_DATA_DIR=/data
EXPOSE 8787
ENTRYPOINT ["/usr/local/bin/littlebox-entrypoint"]
`;

const entrypoint = `#!/bin/sh
set -eu
if [ "$(id -u)" = 0 ]; then
  mkdir -p /data
  chown littlebox:littlebox /data
  chmod 0700 /data
  export HOME=/data
  exec setpriv --reuid=littlebox --regid=littlebox --init-groups --inh-caps=-all -- /usr/local/bin/littlebox-kernel
fi
exec /usr/local/bin/littlebox-kernel
`;

const composeFile = `services:
  kernel:
    build:
      context: .
      dockerfile: Dockerfile
    env_file:
      - .env
    environment:
      LITTLEBOX_BIND: "0.0.0.0"
      LITTLEBOX_KERNEL_PORT: "8787"
      LITTLEBOX_DATA_DIR: /data
      LITTLEBOX_PUBLIC_ORIGIN: http://localhost:\u0024{LITTLEBOX_PORT:-8787}
      LITTLEBOX_HOST_SUFFIX: localhost
    ports:
      - "127.0.0.1:\u0024{LITTLEBOX_PORT:-8787}:8787"
    volumes:
      - ./data:/data
`;

const packageManifest = `{
  "name": "littlebox-selfhosted-app",
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

const buildScript = `import { access, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { build } from 'esbuild';

const root = process.cwd();
const artifactTemplate = JSON.parse(await readFile(path.join(root, 'littlebox/artifact.json'), 'utf8'));
const entrypoint = await exists(path.join(root, 'app/src/main.ts')) ? 'src/main.ts' : 'app.js';
if (!await exists(path.join(root, 'app', entrypoint))) throw new Error('Expected app/src/main.ts or app/app.js.');
const result = await build({
  entryPoints: [path.join(root, 'app', entrypoint)],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'browser',
  target: 'es2022',
  outdir: 'out',
});
const script = result.outputFiles.find((file) => file.path.endsWith('.js'))?.text;
if (!script) throw new Error('The app build did not produce JavaScript.');
const importedCss = result.outputFiles.find((file) => file.path.endsWith('.css'))?.text ?? '';
const runtimeTag = artifactTemplate.files['index.html'].match(/<script data-littlebox-runtime>[\\s\\S]*?<\\/script>/)?.[0];
if (!runtimeTag) throw new Error('The exported artifact does not contain the Littlebox runtime.');
const body = await readOptional(path.join(root, 'app/index.html')) ?? '<main id=app></main>';
const sourceStyles = await readOptional(path.join(root, 'app/styles.css')) ?? '';
const styles = [sourceStyles, importedCss].filter(Boolean).join('\\n');
const safeScript = script.replaceAll('</script', '<\\\\/script');
const html = '<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>'
  + styles + '</style></head><body>' + body + runtimeTag + '<script type="module">' + safeScript + '</script></body></html>';
artifactTemplate.files = { 'index.html': html };
await writeFile(path.join(root, 'littlebox/artifact.json'), JSON.stringify(artifactTemplate, null, 2));
console.log('Built littlebox/artifact.json');

async function exists(file) {
  try { await access(file); return true; } catch { return false; }
}
async function readOptional(file) {
  try { return await readFile(file, 'utf8'); } catch (error) { if (error.code === 'ENOENT') return null; throw error; }
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
LITTLEBOX_PORT=8787
LITTLEBOX_ADMIN_KEY=$admin_key
EOF
fi
set -a
. ./.env
set +a
port=\u0024{LITTLEBOX_PORT:-8787}
if [ ! -d node_modules ]; then npm install; fi
npm run build
docker compose up -d --build
ready=0
for attempt in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  if curl --fail --silent "http://127.0.0.1:\u0024port/_littlebox/health" >/dev/null; then
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

This folder contains the app source, its built static files, and a local copy of the Littlebox Kernel. The Kernel stores the live SQLite database under \u0060data/\u0060 and does not need an object store.

Install Node.js 20 or newer, npm, Docker, and curl before starting.

## Start

Run \u0060./start.sh\u0060. The script installs the pinned build tool, rebuilds the app, starts the Kernel, and loads the app. It creates \u0060.env\u0060 with a local admin key the first time it runs. Open the URL printed by the script. Docker publishes the Kernel only on 127.0.0.1, and the process drops to the unprivileged user \u0060littlebox\u0060 after locking \u0060data/\u0060 to that user.

Edit the files under \u0060app/\u0060, then run \u0060npm run build\u0060 and \u0060npm run deploy\u0060. The Kernel serves the new artifact at the same local app URL.

Run \u0060docker compose down\u0060 to stop the Kernel. The app database and its backups remain under \u0060data/\u0060.

## Where the app lives

Everything the app is sits in one folder, \u0060data/deployments/<id>/\u0060: \u0060deployment.json\u0060, the built \u0060artifact/\u0060, \u0060database.sqlite\u0060, the accounts in \u0060identity.sqlite\u0060, Usage and Signals in \u0060usage.sqlite\u0060, uploads in \u0060files/\u0060 and \u0060backups/\u0060. The owner account is in \u0060data/owners.sqlite\u0060 and the project registry in \u0060data/mcp.sqlite\u0060.

Set \u0060LITTLEBOX_COPY_DIR\u0060 to a directory on another disk and the Kernel copies \u0060data/\u0060 there every hour, keeping the last 24 copies and one a day for 30 days. To move to a new machine, start from an empty data directory and run \u0060LITTLEBOX_DATA_DIR=data cargo run --release --manifest-path kernel-rs/Cargo.toml -- restore --from <copy directory>\u0060, then start the Kernel. Restore checks every checksum and signs everyone out.

## Run without Docker

Install Rust 1.91 or newer as well. Create \u0060.env\u0060 from \u0060.env.example\u0060 with a long random \u0060LITTLEBOX_ADMIN_KEY\u0060, then start the Kernel from this folder:

\u0060\u0060\u0060sh
npm install
npm run build
set -a; . ./.env; set +a
LITTLEBOX_DATA_DIR=data LITTLEBOX_KERNEL_PORT=\u0024{LITTLEBOX_PORT:-8787} LITTLEBOX_PUBLIC_ORIGIN=http://localhost:\u0024{LITTLEBOX_PORT:-8787} cargo run --release --locked --manifest-path kernel-rs/Cargo.toml
\u0060\u0060\u0060

When it answers on \u0060/_littlebox/health\u0060, run \u0060npm run deploy\u0060 in a second terminal. The database lives under \u0060data/\u0060 here too.

## Changing the database

\u0060app/schema.ts\u0060 is the Database Model the Littlebox editor used to plan migrations. This export has no migration generator, so editing it changes nothing. Change \u0060app/schema.sql\u0060 instead, and only in ways the Kernel accepts on every deploy: new tables and indexes with \u0060IF NOT EXISTS\u0060. Back up \u0060data/\u0060 before any change to an existing table.

Keep \u0060.env\u0060 private. It contains the owner key for this local Kernel. The exported app source and static artifact contain no Littlebox-hosted credentials.

## App data

An export made with \u0060--with-data\u0060 holds the app's data in \u0060littlebox/package/\u0060. \u0060manifest.json\u0060 lists the database, the accounts, the files and, when the app recorded any, its Usage and Signals, with the size and SHA-256 of each, and names the published revision they came from. The first \u0060npm run deploy\u0060 imports them. It uploads each part in 64 KiB blocks and records its progress in \u0060littlebox/import-state.json\u0060, so running it again after an interruption continues where it stopped. The Kernel checks every checksum, the accounts each file names and the free space before it creates the app, and refuses a package for an app that already exists. If the script stops after the Kernel created the app, running it again returns the same receipt. Later runs publish the code and keep the data.

Accounts keep their IDs, password hashes, verification state and suspensions, and the app keeps its visibility and signup policy. Sessions, open email links and API keys stay behind, so everyone signs in again. The package holds personal data and password hashes. Keep this folder private and out of Git, and delete \u0060littlebox/package/\u0060 once the app runs.

Exports made before packages hold \u0060littlebox/initial-database.sqlite\u0060, \u0060littlebox/initial-identity.sqlite\u0060, \u0060littlebox/initial-files.portable\u0060 and \u0060littlebox/initial-signup-policy.json\u0060 instead, at most 16 MiB together. The deploy script still imports them the same way.
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
 * the package's visibility and carries `littlebox/package/manifest.json`; the
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
    const entries = projectExportEntries(project, artifact, kernelFiles, { visibility: manifest.visibility } as PortableProjectState);
    entries.splice(entries.findIndex((entry) => entry.path === "littlebox/artifact.json") + 1, 0,
      { path: "littlebox/package/manifest.json", contents: JSON.stringify(manifest, null, 2), mode: 0o600 });
    return entries;
  }
  if (artifact.version !== 1 || artifact.entrypoint !== "index.html" || !artifact.files["index.html"]) {
    throw new Error("The export requires a valid version 1 Build Artifact.");
  }
  // scripts/build.mjs finds the runtime by this marker; without it every rebuild would fail.
  if (!artifact.files["index.html"].includes(`<script ${runtimeScriptAttribute}>`)) {
    throw new Error("The Build Artifact does not mark its Littlebox runtime script. Rebuild the project before exporting.");
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
  const normalizedArtifact = {
    version: artifact.version,
    entrypoint: artifact.entrypoint,
    files: artifact.files,
  };
  const entries: TarEntry[] = [
    { path: "README.md", contents: readme },
    { path: "Dockerfile", contents: dockerfile },
    { path: "entrypoint.sh", contents: entrypoint, mode: 0o755 },
    { path: "compose.yaml", contents: composeFile },
    { path: ".env.example", contents: "LITTLEBOX_PORT=8787\nLITTLEBOX_ADMIN_KEY=replace-me\n" },
    { path: ".gitignore", contents: ".env\ndata/\nnode_modules/\nlittlebox/initial-*\nlittlebox/package/\nlittlebox/import-state.json\n" },
    { path: "package.json", contents: packageManifest },
    { path: "scripts/build.mjs", contents: buildScript },
    { path: "scripts/deploy.mjs", contents: deployScript },
    { path: "start.sh", contents: startScript, mode: 0o755 },
    { path: "littlebox/project.json", contents: JSON.stringify({
      id: project.id,
      name: project.name,
      slug: project.slug,
      manifest: portableState?.visibility ? { ...project.manifest, visibility: portableState.visibility } : project.manifest,
      databaseModel: project.databaseModel,
    }, null, 2) },
    { path: "littlebox/artifact.json", contents: JSON.stringify(normalizedArtifact, null, 2) },
    ...(portableState?.initialDatabase ? [{ path: "littlebox/initial-database.sqlite", contents: portableState.initialDatabase, mode: 0o600 }] : []),
    ...(portableState?.initialIdentity ? [{ path: "littlebox/initial-identity.sqlite", contents: portableState.initialIdentity, mode: 0o600 }] : []),
    ...(portableState?.initialFiles ? [{ path: "littlebox/initial-files.portable", contents: portableState.initialFiles, mode: 0o600 }] : []),
    ...(portableState?.initialSignupPolicy ? [{ path: "littlebox/initial-signup-policy.json", contents: JSON.stringify(portableState.initialSignupPolicy), mode: 0o600 }] : []),
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

