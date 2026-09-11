import { readFile, readdir, writeFile } from 'node:fs/promises';
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

try {
  await rpc('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'littlebox-selfhost', version: '0.1.0' } });
  await rpc('notifications/initialized', undefined, true);
  const project = JSON.parse(await readFile(path.join(root, 'littlebox/project.json'), 'utf8'));
  const artifact = JSON.parse(await readFile(path.join(root, 'littlebox/artifact.json'), 'utf8'));
  const files = await readProjectFiles(path.join(root, 'app'));
  await call('project_create', { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const stateMarkerPath = path.join(root, 'littlebox/deployment.json');
  const remote = await call('project_inspect', { projectId: project.id });
  const payload = {
    project: { id: project.id, name: project.name, slug: project.slug, files, manifest: project.manifest },
    visibility: project.manifest.visibility || 'private', artifact,
  };
  if (!remote.deployment) {
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
    deployment = await call('release_apply', { planId: plan.planId });
  } finally {
    try { await call('transfer_cancel', { transferId: transfer.transferId }); }
    catch (error) { console.warn('Could not release the upload: ' + error.message); }
  }
  await writeFile(stateMarkerPath, JSON.stringify({ projectId: project.id, port, deploymentId: deployment.id }, null, 2) + '\n');
  console.log('App ready at ' + deployment.url);
} finally {
  await fetch(base + '/_littlebox/owner/logout', { method: 'POST', headers: { cookie } });
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
