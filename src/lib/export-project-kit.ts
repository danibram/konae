import { createTar, normalizeArchivePath, type TarEntry } from "./tar";
import { normalizeProjectPath } from "./project-files";
import type { BuildArtifact } from "../runtime/artifact";
import type { MiniAppProject } from "./types";
import type { PortableProjectState } from "../deployments/client";

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
COPY --from=build /src/target/release/littlebox-kernel /usr/local/bin/littlebox-kernel
EXPOSE 8787
ENTRYPOINT ["littlebox-kernel"]
`;

const composeFile = `services:
  kernel:
    build:
      context: .
      dockerfile: Dockerfile
    env_file:
      - .env
    environment:
      LITTLEBOX_KERNEL_PORT: "8787"
      LITTLEBOX_DATA_DIR: /data
      LITTLEBOX_PUBLIC_ORIGIN: http://localhost:\u0024{LITTLEBOX_PORT:-8787}
      LITTLEBOX_HOST_SUFFIX: localhost
    ports:
      - "\u0024{LITTLEBOX_PORT:-8787}:8787"
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
const runtimeTag = artifactTemplate.files['index.html'].match(/<script>[\\s\\S]*?<\\/script>/)?.[0];
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

const deployScript = `import { readFile, readdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

const root = process.cwd();
const envText = await readFile(path.join(root, '.env'), 'utf8');
const env = Object.fromEntries(envText.split(/\\r?\\n/).filter((line) => line && !line.startsWith('#')).map((line) => {
  const split = line.indexOf('=');
  return [line.slice(0, split), line.slice(split + 1)];
}));
const port = env.LITTLEBOX_PORT || '8787';
const project = JSON.parse(await readFile(path.join(root, 'littlebox/project.json'), 'utf8'));
const artifact = JSON.parse(await readFile(path.join(root, 'littlebox/artifact.json'), 'utf8'));
const files = await readProjectFiles(path.join(root, 'app'));
const stateMarkerPath = path.join(root, 'littlebox/deployment.json');
let portableStatePending = true;
try {
  const previous = JSON.parse(await readFile(stateMarkerPath, 'utf8'));
  portableStatePending = previous.projectId !== project.id || previous.port !== port;
} catch (error) {
  if (error.code !== 'ENOENT') throw error;
}
const payload = {
  project: { id: project.id, name: project.name, slug: project.slug, files, manifest: project.manifest },
  visibility: project.manifest.visibility || 'public',
  artifact,
};
if (portableStatePending) {
  try {
    payload.initialDatabase = (await readFile(path.join(root, 'littlebox/initial-database.sqlite'))).toString('base64');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  for (const [field, filename] of [['initialIdentity', 'initial-identity.sqlite'], ['initialFiles', 'initial-files.portable']]) {
    try {
      payload[field] = (await readFile(path.join(root, 'littlebox', filename))).toString('base64');
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  }
}
let response = await fetch('http://127.0.0.1:' + port + '/_littlebox/admin/deploy', {
  method: 'POST',
  headers: { 'content-type': 'application/json', 'x-littlebox-admin-key': env.LITTLEBOX_ADMIN_KEY },
  body: JSON.stringify(payload),
});
let body = await response.json().catch(() => ({}));
if (response.status === 409 && body.error === 'Portable state can only be imported on the first deployment.'
    && ('initialDatabase' in payload || 'initialIdentity' in payload || 'initialFiles' in payload)) {
  delete payload.initialDatabase;
  delete payload.initialIdentity;
  delete payload.initialFiles;
  response = await fetch('http://127.0.0.1:' + port + '/_littlebox/admin/deploy', {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-littlebox-admin-key': env.LITTLEBOX_ADMIN_KEY },
    body: JSON.stringify(payload),
  });
  body = await response.json().catch(() => ({}));
}
if (!response.ok || !body.result?.url) throw new Error(body.error || 'Kernel deployment failed.');
await writeFile(stateMarkerPath, JSON.stringify({ projectId: project.id, port, deploymentId: body.result.id }, null, 2) + '\\n');
console.log('App ready at ' + body.result.url);

async function readProjectFiles(directory, prefix = '') {
  const result = {};
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const relative = prefix ? prefix + '/' + entry.name : entry.name;
    if (entry.isDirectory()) Object.assign(result, await readProjectFiles(path.join(directory, entry.name), relative));
    else if (entry.isFile()) result[relative] = await readFile(path.join(directory, entry.name), 'utf8');
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

Run \u0060./start.sh\u0060. The script installs the pinned build tool, rebuilds the app, starts the Kernel, and loads the app. It creates \u0060.env\u0060 with a local admin key the first time it runs. Open the URL printed by the script.

Edit the files under \u0060app/\u0060, then run \u0060npm run build\u0060 and \u0060npm run deploy\u0060. The Kernel serves the new artifact at the same local app URL.

Run \u0060docker compose down\u0060 to stop the Kernel. The app database and its backups remain under \u0060data/\u0060.

Keep \u0060.env\u0060 private. It contains the owner key for this local Kernel. The exported app source and static artifact contain no Littlebox-hosted credentials.

The optional \u0060littlebox/initial-database.sqlite\u0060, \u0060littlebox/initial-identity.sqlite\u0060, and \u0060littlebox/initial-files.portable\u0060 files hold portable app state supplied during export. User password hashes and file attachments transfer with the app, while the target Kernel starts new sessions.
`;

export function buildProjectExport(
  project: MiniAppProject,
  artifact: BuildArtifact,
  kernelFiles: readonly ExportFile[],
  state?: Uint8Array | PortableProjectState,
): { archive: Uint8Array; entries: TarEntry[] } {
  if (artifact.version !== 1 || artifact.entrypoint !== "index.html" || !artifact.files["index.html"]) {
    throw new Error("The export requires a valid version 1 Build Artifact.");
  }
  const portableState = state instanceof Uint8Array ? { initialDatabase: state } : state;
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
    { path: "compose.yaml", contents: composeFile },
    { path: ".env.example", contents: "LITTLEBOX_PORT=8787\nLITTLEBOX_ADMIN_KEY=replace-me\n" },
    { path: ".gitignore", contents: ".env\ndata/\n" },
    { path: "package.json", contents: packageManifest },
    { path: "scripts/build.mjs", contents: buildScript },
    { path: "scripts/deploy.mjs", contents: deployScript },
    { path: "start.sh", contents: startScript, mode: 0o755 },
    { path: "littlebox/project.json", contents: JSON.stringify({
      id: project.id,
      name: project.name,
      slug: project.slug,
      manifest: project.manifest,
      databaseModel: project.databaseModel,
    }, null, 2) },
    { path: "littlebox/artifact.json", contents: JSON.stringify(normalizedArtifact, null, 2) },
    ...(portableState?.initialDatabase ? [{ path: "littlebox/initial-database.sqlite", contents: portableState.initialDatabase }] : []),
    ...(portableState?.initialIdentity ? [{ path: "littlebox/initial-identity.sqlite", contents: portableState.initialIdentity }] : []),
    ...(portableState?.initialFiles ? [{ path: "littlebox/initial-files.portable", contents: portableState.initialFiles }] : []),
    ...files,
    ...kernelFiles.map((file) => ({ path: safeProjectPath(file.path), contents: file.contents })),
  ];
  const uniquePaths = new Set<string>();
  for (const entry of entries) {
    const path = normalizeArchivePath(entry.path);
    if (uniquePaths.has(path)) throw new Error(`Duplicate export path: ${path}`);
    uniquePaths.add(path);
  }
  return { archive: createTar(entries), entries };
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

