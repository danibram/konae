import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { appFetch, buildKernel, release, startKernel, type Kernel } from "./kernel-harness";

// An app in maintenance takes no writes. The owner puts it in and takes it out
// with maintenance_set, and kernel-rs/kernel/src/maintenance.rs keeps the state
// in apps/<id>/.konae/maintenance.json, so a restart keeps it.
let root: string;
let kernel: Kernel;

const actions = { version: 1, actions: {
  "notes.add": { kind: "mutation", access: "anonymous", statement: "INSERT INTO notes(body) VALUES(?1)", bindings: [{ source: "input", key: "body", type: "string", maxLength: 200 }] },
  "notes.list": { kind: "query", access: "anonymous", statement: "SELECT body FROM notes ORDER BY id LIMIT 50", bindings: [] },
} };
const payload = (id: string) => ({
  project: { id, name: "Paused", slug: "paused", manifest: { version: 1, visibility: "public" }, files: { "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);", "actions.json": JSON.stringify(actions) } },
  visibility: "public",
  artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Paused app</main>" } },
});

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "konae-maintenance-"));
  const port = Number(process.env.KONAE_MAINTENANCE_TEST_PORT) || 19700 + Math.floor(Math.random() * 90);
  kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: "maintenance-setup-secret", ownerEmail: "owner@maintenance.test" });
}, 300_000);

afterAll(async () => { await kernel?.stop(); if (root) await rm(root, { recursive: true, force: true }); });

test("an owner puts an app in maintenance, where it serves its page, refuses writes and owner changes and answers reads, and takes it out", async () => {
  const id = "maintenance_app";
  await kernel.owner.call("project_create", { projectId: id, name: "Paused" });
  const { receipt } = await release(kernel.owner, payload(id), id);
  const invoke = (name: string, input: object = {}) => appFetch(kernel, receipt.hostname, "/_konae/mcp", {
    method: "POST",
    headers: { accept: "application/json, text/event-stream", "MCP-Protocol-Version": "2025-11-25" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "actions_invoke", arguments: { name, input } } }),
  });
  expect((await invoke("notes.add", { body: "before" })).status).toBe(200);

  const paused = await kernel.owner.call<{ maintenance: boolean; since: number }>("maintenance_set", { deploymentId: receipt.id, on: true });
  expect(paused.maintenance).toBe(true);
  await kernel.stop();
  await kernel.start();
  expect(await kernel.owner.call("maintenance_read", { deploymentId: receipt.id })).toEqual({ maintenance: true, since: paused.since, unreadable: false, installation: null });

  const page = await appFetch(kernel, receipt.hostname, "/", { headers: { accept: "text/html" } });
  expect(page.status).toBe(503);
  expect(page.headers.get("retry-after")).toBe("300");
  expect(await page.text()).toContain("Paused is in maintenance");

  const write = await invoke("notes.add", { body: "during" });
  expect(write.status).toBe(503);
  expect(await write.json()).toEqual({ error: "This app is in maintenance. It accepts no writes, signups, recovery, administrative changes or email until it comes out of maintenance.", maintenance: true });

  await expect(kernel.owner.call("signup_policy_set", { deploymentId: receipt.id, mode: "closed" })).rejects.toThrow("This app is in maintenance.");
  const inspected = await kernel.owner.call<{ deployment: { id: string } }>("project_inspect", { projectId: id });
  expect(inspected.deployment.id).toBe(receipt.id);
  const rows = await kernel.owner.call<{ rows: { body: string }[] }>("database_execute", { deploymentId: receipt.id, operation: "browse", input: { tableName: "notes" } }).catch((error: Error) => error);
  expect(rows).not.toBeInstanceOf(Error);

  // Taking the app out is the one change a paused app takes, and it writes again.
  expect((await kernel.owner.call<{ maintenance: boolean }>("maintenance_set", { deploymentId: receipt.id, on: false })).maintenance).toBe(false);
  expect((await invoke("notes.add", { body: "after" })).status).toBe(200);
  expect((await appFetch(kernel, receipt.hostname, "/", { headers: { accept: "text/html" } })).status).toBe(200);
}, 120_000);
