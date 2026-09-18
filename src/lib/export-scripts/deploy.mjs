import { open, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import path from 'node:path';

const root = process.cwd();
const envText = await readFile(path.join(root, '.env'), 'utf8');
const env = Object.fromEntries(envText.split(/\r?\n/).filter(line => line && !line.startsWith('#')).map(line => {
  const split = line.indexOf('=');
  return [line.slice(0, split), line.slice(split + 1)];
}));
const port = env.LITTLEBOX_PORT || '8787';
const base = 'http://127.0.0.1:' + port;
const authInput = { email: 'owner@selfhost.local', password: env.LITTLEBOX_ADMIN_KEY, setupKey: env.LITTLEBOX_ADMIN_KEY };
let response = await fetch(base + '/_littlebox/owner/login', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(authInput) });
if (!response.ok) response = await fetch(base + '/_littlebox/owner/signup', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(authInput) });
if (!response.ok) throw Error('Could not authenticate the self-hosted owner.');
const cookie = response.headers.get('set-cookie')?.split(';')[0];
if (!cookie) throw Error('The owner session is missing.');

async function rpc(method, params, notification = false) {
  const id = randomUUID();
  const response = await fetch(base + '/mcp', {
    method: 'POST', headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream', cookie, 'MCP-Protocol-Version': '2025-11-25' },
    body: JSON.stringify({ jsonrpc: '2.0', ...(!notification ? { id } : {}), method, ...(params ? { params } : {}) }),
  });
  if (notification && response.status === 202) return;
  const body = await response.json();
  if (!response.ok || body.error || body.id !== id) throw Error(body.error?.message || body.error || 'MCP request failed.');
  return body.result;
}
async function call(name, args = {}) {
  const result = await rpc('tools/call', { name, arguments: args });
  if (result.isError) throw Error(result.content?.[0]?.text || 'MCP tool failed.');
  return result.structuredContent.result;
}
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const importStatePath = path.join(root, 'littlebox/import-state.json');

try {
  await rpc('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'littlebox-selfhost', version: '0.1.0' } });
  await rpc('notifications/initialized', undefined, true);
  const project = JSON.parse(await readFile(path.join(root, 'littlebox/project.json'), 'utf8'));
  const artifact = JSON.parse(await readFile(path.join(root, 'littlebox/artifact.json'), 'utf8'));
  const files = await readProjectFiles(path.join(root, 'app'));
  await call('project_create', { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const stateMarkerPath = path.join(root, 'littlebox/deployment.json');
  const packageManifest = await readJson(path.join(root, 'littlebox/package/manifest.json'));
  let importState = await readJson(importStatePath);
  const remote = await call('project_inspect', { projectId: project.id });
  const payload = {
    project: { id: project.id, name: project.name, slug: project.slug, files, manifest: project.manifest },
    visibility: project.manifest.visibility || 'private', artifact,
  };
  if (remote.deployment && importState?.planId) {
    // The Kernel created the app before this script recorded it. The applied plan returns its receipt.
    const deployment = await call('release_apply', { planId: importState.planId });
    await finishImport(importState);
    await writeFile(stateMarkerPath, JSON.stringify({ projectId: project.id, port, deploymentId: deployment.id }, null, 2) + '\n');
    console.log('App already imported at ' + deployment.url);
  } else {
    if (!remote.deployment && packageManifest) {
      if (importState?.packageId !== packageManifest.packageId) importState = { packageId: packageManifest.packageId, components: {} };
      for (const content of packageManifest.contents) {
        importState.components[content.name] = await uploadComponent(content, importState.components[content.name], project.id);
        await writeFile(importStatePath, JSON.stringify(importState, null, 2) + '\n', { mode: 0o600 });
      }
      payload.visibility = packageManifest.visibility;
      payload.initialPackage = { manifest: packageManifest, components: importState.components };
    } else if (!remote.deployment) {
      for (const [field, filename] of [['initialDatabase', 'initial-database.sqlite'], ['initialIdentity', 'initial-identity.sqlite'], ['initialFiles', 'initial-files.portable']]) {
        try { payload[field] = (await readFile(path.join(root, 'littlebox', filename))).toString('base64'); }
        catch (error) { if (error.code !== 'ENOENT') throw error; }
      }
      try { payload.initialSignupPolicy = JSON.parse(await readFile(path.join(root, 'littlebox/initial-signup-policy.json'), 'utf8')); }
      catch (error) { if (error.code !== 'ENOENT') throw error; }
    }
    const bytes = Buffer.from(JSON.stringify(payload));
    const transfer = await call('transfer_begin', { projectId: project.id, size: bytes.length, sha256: hash(bytes) });
    let deployment;
    try {
      for (let offset = 0; offset < bytes.length; offset += transfer.chunkBytes) {
        await call('transfer_write', { transferId: transfer.transferId, offset, data: bytes.subarray(offset, offset + transfer.chunkBytes).toString('base64') });
      }
      await call('transfer_complete', { transferId: transfer.transferId });
      const plan = await call('release_plan', { transferId: transfer.transferId });
      if (payload.initialPackage) {
        importState.planId = plan.planId;
        await writeFile(importStatePath, JSON.stringify(importState, null, 2) + '\n', { mode: 0o600 });
      }
      deployment = await call('release_apply', { planId: plan.planId });
    } finally {
      try { await call('transfer_cancel', { transferId: transfer.transferId }); }
      catch (error) { console.warn('Could not release the upload: ' + error.message); }
    }
    if (payload.initialPackage) await finishImport(importState);
    await writeFile(stateMarkerPath, JSON.stringify({ projectId: project.id, port, deploymentId: deployment.id }, null, 2) + '\n');
    console.log('App ready at ' + deployment.url);
  }
} finally {
  await fetch(base + '/_littlebox/owner/logout', { method: 'POST', headers: { cookie } });
}

async function readJson(file) {
  try { return JSON.parse(await readFile(file, 'utf8')); }
  catch (error) { if (error.code === 'ENOENT') return undefined; throw error; }
}

// Uploads one package component in 64 KiB blocks read from disk. An upload
// the Kernel still holds continues from its offset.
async function uploadComponent(content, transferId, projectId) {
  let offset = 0;
  if (transferId) {
    try {
      const status = await call('transfer_status', { transferId });
      if (status.complete) return transferId;
      offset = status.offset;
    } catch { transferId = undefined; }
  }
  if (!transferId) {
    transferId = (await call('transfer_begin', { projectId, size: content.size, sha256: content.sha256, component: content.name })).transferId;
  }
  const names = { database: 'database.sqlite', identity: 'identity.sqlite', files: 'files.tar' };
  const file = await open(path.join(root, 'littlebox/package', names[content.name]), 'r');
  try {
    const buffer = Buffer.alloc(64 * 1024);
    while (offset < content.size) {
      const { bytesRead } = await file.read(buffer, 0, Math.min(buffer.length, content.size - offset), offset);
      if (bytesRead === 0) throw Error('littlebox/package/' + names[content.name] + ' is shorter than its manifest size.');
      await call('transfer_write', { transferId, offset, data: buffer.subarray(0, bytesRead).toString('base64') });
      offset += bytesRead;
    }
  } finally {
    await file.close();
  }
  await call('transfer_complete', { transferId });
  return transferId;
}

async function finishImport(state) {
  for (const transferId of Object.values(state.components ?? {})) {
    try { await call('transfer_cancel', { transferId }); } catch {}
  }
  await rm(importStatePath, { force: true });
}

async function readProjectFiles(directory, prefix = '') {
  const result = {};
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const relative = prefix ? prefix + '/' + entry.name : entry.name;
    if (entry.isDirectory()) Object.assign(result, await readProjectFiles(path.join(directory, entry.name), relative));
    else if (entry.isFile()) result[relative] = await readFile(path.join(directory, entry.name), 'utf8');
  }
  return result;
}
