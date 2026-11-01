import { open, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { request } from 'node:http';
import path from 'node:path';

const root = process.cwd();
const envText = await readFile(path.join(root, '.env'), 'utf8');
const env = Object.fromEntries(envText.split(/\r?\n/).filter(line => line && !line.startsWith('#')).map(line => {
  const split = line.indexOf('=');
  return [line.slice(0, split), line.slice(split + 1)];
}));
const port = env.KONAE_PORT || '8787';
// The script always reaches the Kernel on this machine. With a public origin in
// .env, it names that origin's host, because the Kernel accepts owner sign-in
// and administration only there. fetch would drop a Host header.
const kernelHost = env.KONAE_PUBLIC_ORIGIN ? new URL(env.KONAE_PUBLIC_ORIGIN).host : '127.0.0.1:' + port;
const authInput = { email: 'owner@selfhost.local', password: env.KONAE_ADMIN_KEY, setupKey: env.KONAE_ADMIN_KEY };
let response = await post('/_konae/owner/login', { 'content-type': 'application/json' }, JSON.stringify(authInput));
if (!response.ok) response = await post('/_konae/owner/signup', { 'content-type': 'application/json' }, JSON.stringify(authInput));
if (!response.ok) throw Error('Could not authenticate the self-hosted owner: ' + (response.json()?.error?.replace(/\.$/, '') || 'HTTP ' + response.status) + '.');
const cookie = [response.headers['set-cookie']].flat()[0]?.split(';')[0];
if (!cookie) throw Error('The owner session is missing.');

function post(pathname, headers, body = '') {
  return new Promise((resolve, reject) => {
    const outgoing = request({ host: '127.0.0.1', port, path: pathname, method: 'POST', headers: { ...headers, host: kernelHost, 'content-length': Buffer.byteLength(body) } }, incoming => {
      const chunks = [];
      incoming.on('data', chunk => chunks.push(chunk));
      incoming.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf8');
        const status = incoming.statusCode ?? 0;
        resolve({ ok: status >= 200 && status < 300, status, headers: incoming.headers, json: () => { try { return JSON.parse(text); } catch { return undefined; } } });
      });
      incoming.on('error', reject);
    });
    outgoing.on('error', reject);
    outgoing.end(body);
  });
}

async function rpc(method, params, notification = false) {
  const id = randomUUID();
  const response = await post('/mcp',
    { 'content-type': 'application/json', accept: 'application/json, text/event-stream', cookie, 'MCP-Protocol-Version': '2025-11-25' },
    JSON.stringify({ jsonrpc: '2.0', ...(!notification ? { id } : {}), method, ...(params ? { params } : {}) }));
  if (notification && response.status === 202) return;
  const body = response.json() ?? {};
  if (!response.ok || body.error || body.id !== id) throw Error(body.error?.message || body.error || 'MCP request failed.');
  return body.result;
}
async function call(name, args = {}) {
  const result = await rpc('tools/call', { name, arguments: args });
  if (result.isError) throw Error(result.content?.[0]?.text || 'MCP tool failed.');
  return result.structuredContent.result;
}
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const importStatePath = path.join(root, 'konae/import-state.json');

try {
  await rpc('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'konae-selfhost', version: '0.1.0' } });
  await rpc('notifications/initialized', undefined, true);
  const project = JSON.parse(await readFile(path.join(root, 'konae/project.json'), 'utf8'));
  const artifact = JSON.parse(await readFile(path.join(root, 'konae/artifact.json'), 'utf8'));
  const files = await readProjectFiles(path.join(root, 'app'));
  await call('project_create', { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const stateMarkerPath = path.join(root, 'konae/deployment.json');
  const packageManifest = await readJson(path.join(root, 'konae/package/manifest.json'));
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
    await domainInstructions(deployment.id);
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
      if (project.domain) payload.initialDomain = project.domain;
      for (const [field, filename] of [['initialDatabase', 'initial-database.sqlite'], ['initialIdentity', 'initial-identity.sqlite'], ['initialFiles', 'initial-files.portable']]) {
        try { payload[field] = (await readFile(path.join(root, 'konae', filename))).toString('base64'); }
        catch (error) { if (error.code !== 'ENOENT') throw error; }
      }
      try { payload.initialSignupPolicy = JSON.parse(await readFile(path.join(root, 'konae/initial-signup-policy.json'), 'utf8')); }
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
      // A schema change that loses data applies only with its confirmation,
      // which the plan prints and KONAE_CONFIRM_SCHEMA in the environment gives back.
      const confirmation = plan.schema?.confirmation;
      if (confirmation && process.env.KONAE_CONFIRM_SCHEMA !== confirmation) {
        console.log('This release changes the database and loses data:');
        for (const loss of plan.schema.losses) console.log('  ' + loss.sentence);
        throw Error('Nothing was published. To apply it, run KONAE_CONFIRM_SCHEMA=' + confirmation + ' npm run deploy.');
      }
      deployment = await call('release_apply', { planId: plan.planId, ...(confirmation ? { confirmSchema: confirmation } : {}) });
    } finally {
      try { await call('transfer_cancel', { transferId: transfer.transferId }); }
      catch (error) { console.warn('Could not release the upload: ' + error.message); }
    }
    if (payload.initialPackage) await finishImport(importState);
    await writeFile(stateMarkerPath, JSON.stringify({ projectId: project.id, port, deploymentId: deployment.id }, null, 2) + '\n');
    console.log('App ready at ' + deployment.url);
    await domainInstructions(deployment.id);
  }
} finally {
  await post('/_konae/owner/logout', { cookie });
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
  // The same names as packageComponentFiles in src/lib/portable-package.ts.
  const names = { database: 'database.sqlite', identity: 'identity.sqlite', files: 'files.tar', usage: 'usage.sqlite' };
  const file = await open(path.join(root, 'konae/package', names[content.name]), 'r');
  try {
    const buffer = Buffer.alloc(64 * 1024);
    while (offset < content.size) {
      const { bytesRead } = await file.read(buffer, 0, Math.min(buffer.length, content.size - offset), offset);
      if (bytesRead === 0) throw Error('konae/package/' + names[content.name] + ' is shorter than its manifest size.');
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
    // Installed packages are build input, not app source, as in the CLI.
    if (entry.name === 'node_modules') continue;
    const relative = prefix ? prefix + '/' + entry.name : entry.name;
    if (entry.isDirectory()) Object.assign(result, await readProjectFiles(path.join(directory, entry.name), relative));
    else if (entry.isFile()) result[relative] = await readFile(path.join(directory, entry.name), 'utf8');
  }
  return result;
}

// KONAE_APP_DOMAIN in .env names the app's own hostname. Each run asks the
// Kernel to verify it until the TXT record holds the token it issued.
async function domainInstructions(deploymentId) {
  const wanted = env.KONAE_APP_DOMAIN?.trim().toLowerCase().replace(/\.$/, '');
  let domain = await call('domain_read', { deploymentId });
  if (wanted && (domain.hostname !== wanted || domain.status !== 'active')) {
    // A record that still holds another installation's token is refused. The
    // app is already published, so the run reports it instead of failing.
    try { domain = await call('domain_set', { deploymentId, hostname: wanted }); }
    catch (error) {
      console.log(error.message);
      domain = await call('domain_read', { deploymentId });
    }
  }
  if (!domain.hostname) return;
  if (domain.status === 'active') {
    console.log('The app also answers at ' + domain.url + '. Your proxy must terminate TLS for ' + domain.hostname + ' and forward Host unchanged.');
    return;
  }
  console.log('Custom domain ' + domain.hostname + ' needs verification on this Kernel.');
  console.log('Set TXT ' + domain.record.name + ' to ' + domain.record.value + '. A value another installation needs can stay next to it.');
  if (domain.dns) console.log(domain.dns);
  console.log('Point the hostname at this server and configure TLS with Host forwarded unchanged.');
  if (wanted) console.log('Then run npm run deploy again.');
  else console.log('Then set KONAE_APP_DOMAIN=' + domain.hostname + ' in .env and run npm run deploy again.');
}
