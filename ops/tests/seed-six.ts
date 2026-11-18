/** Seed the six pilot apps through their owner and runtime MCP endpoints. */
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import path from "node:path";
import { McpClient, type Transfer } from "../../src/mcp/client";

const [exe, folder] = process.argv.slice(2);
if (!exe || !folder) throw new Error("Usage: bun ops/tests/seed-six.ts BINARY NEW_FOLDER");
await mkdir(folder, { mode: 0o700 });
const socket = createServer().listen(0, "127.0.0.1");
await new Promise<void>(resolve => socket.once("listening", resolve));
const address = socket.address();
assert(address && typeof address === "object");
await new Promise<void>((resolve, reject) => socket.close(err => err ? reject(err) : resolve()));
const url = `http://127.0.0.1:${address.port}`;
const setupKey = "copy-restore-setup-secret";
const password = "copy-restore-password-2026";
const env = {
  PATH: "/usr/bin:/bin", KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(address.port),
  KONAE_PUBLIC_ORIGIN: url, KONAE_SETUP_KEY: setupKey, KONAE_DATA_DIR: path.resolve(folder, "live"),
  KONAE_HOST_SUFFIX: "localhost", KONAE_MAIL_TRANSPORT: "development", TOKIO_WORKER_THREADS: "2",
};
const child = Bun.spawn([path.resolve(exe)], { env, stdout: "ignore", stderr: "inherit" });

async function sign(route: string, email: string, host?: string) {
  const reply = await fetch(url + route, {
    method: "POST", headers: { "content-type": "application/json", ...(host && { host }) },
    body: JSON.stringify({ email, password, ...(!host && { setupKey }) }),
  });
  assert.equal(reply.status, 200);
  return reply.headers.get("set-cookie")!.split(";")[0];
}

function client(route: string, cookie: string, host?: string) {
  return new McpClient(url + route, { fetch: (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("cookie", cookie);
    if (host) headers.set("host", host);
    return fetch(input, { ...init, headers });
  } });
}

const files = {
  "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, title TEXT NOT NULL, user_id TEXT NOT NULL);",
  "actions.json": JSON.stringify({ version: 1, actions: {
    "notes.list": { kind: "query", access: "authenticated", statement: "SELECT id,title FROM notes WHERE user_id=?1 LIMIT 50",
      bindings: [{ source: "actor", field: "id" }] },
    "notes.create": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(title,user_id) VALUES(?1,?2)",
      bindings: [{ source: "input", key: "title", type: "string", maxLength: 100 }, { source: "actor", field: "id" }] },
  } }),
};
const attachment = new TextEncoder().encode("Attachment survives the copy");
const apps = [];
try {
  const deadline = Date.now() + 30000;
  while (!(await fetch(url + "/_konae/health").catch(() => null))?.ok) {
    if (child.exitCode !== null || Date.now() > deadline) throw new Error("Pilot service did not start");
    await Bun.sleep(50);
  }
  const owner = client("/mcp", await sign("/_konae/owner/signup", "owner@copy.test"));
  const { can } = await owner.call<{ can: Record<string, boolean> }>("session_inspect", {});
  assert(can.read && can.write && can.publish);
  for (let n = 0; n < 6; n++) {
    const projectId = "fixture_app_" + n;
    const name = "Synthetic profile " + n;
    await owner.call("project_create", { projectId, name });
    const observed = await owner.call<{ sourcesRevision: number }>("project_inspect", { projectId });
    const release = {
      project: { id: projectId, name, slug: "fixture-app-" + n, manifest: { version: 1, visibility: "public" }, files },
      visibility: "public", initialModules: { auth: { on: true }, files: { on: true } },
      artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Synthetic profile</main>" } },
    };
    const upload = await owner.upload(new TextEncoder().encode(JSON.stringify(release)), projectId);
    const plan = await owner.call<{ planId: string }>("release_plan", {
      transferId: upload.transferId, observedSourcesRevision: observed.sourcesRevision,
    });
    const { id, hostname } = await owner.call<{ id: string; hostname: string }>("release_apply", { planId: plan.planId });
    const inspected = await owner.call<{ deployment: { hostname: string } }>("project_inspect", { projectId });
    assert.equal(inspected.deployment.hostname, hostname);
    await owner.cancel(upload.transferId);
    await sign("/_konae/auth/signup", "reader@copy.test", hostname);
    const writer = client("/_konae/mcp", await sign("/_konae/auth/signup", "writer@copy.test", hostname), hostname);
    const title = "Synthetic " + n;
    await writer.call("actions_invoke", { name: "notes.create", input: { title } });
    assert.deepEqual(await writer.call("actions_invoke", { name: "notes.list" }), [{ id: 1, title }]);
    const transfer = await writer.upload(attachment);
    const file = await writer.call<{ id: string }>("files_upload", {
      transferId: transfer.transferId, name: "sample.txt", type: "text/plain", visibility: "private",
    });
    assert((await writer.call<{ id: string }[]>("files_list", {})).some(f => f.id === file.id));
    assert.deepEqual(Buffer.from(await writer.download(await writer.call<Transfer>("files_read", { fileId: file.id }))),
      Buffer.from(attachment));
    await writer.cancel(transfer.transferId);
    apps.push({ id, hostname, projectId, fileId: file.id });
  }
} finally {
  child.kill();
  await child.exited;
}
await writeFile(path.join(folder, "apps.json"), JSON.stringify(apps), { flag: "wx" });
