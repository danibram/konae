import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { appFetch, buildKernel, release, startKernel, type Kernel } from "./kernel-harness";

// An app in maintenance takes no writes. kernel-rs/src/maintenance.rs reads
// deployments/<id>/maintenance.json at startup; no tool writes it yet, so the
// test writes the file while the Kernel is stopped, as an operator would.
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

test("an app in maintenance serves its page, refuses writes and owner changes, and still answers reads", async () => {
  const id = "maintenance_app";
  await kernel.owner.call("project_create", { projectId: id, name: "Paused" });
  const { receipt } = await release(kernel.owner, payload(id), id);
  const invoke = (name: string, input: object = {}) => appFetch(kernel, receipt.hostname, "/_konae/mcp", {
    method: "POST",
    headers: { accept: "application/json, text/event-stream", "MCP-Protocol-Version": "2025-11-25" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "actions_invoke", arguments: { name, input } } }),
  });
  expect((await invoke("notes.add", { body: "before" })).status).toBe(200);

  await kernel.stop();
  await writeFile(path.join(kernel.data, "deployments", receipt.id, "maintenance.json"), JSON.stringify({ since: Date.now() }));
  await kernel.start();

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
}, 120_000);
