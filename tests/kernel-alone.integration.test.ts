import { afterAll, beforeAll, expect, test } from "bun:test";
import { existsSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { appFetch, buildKernel, release, startKernel, type Kernel } from "./kernel-harness";

// The kernel binary serves a data directory of app folders with no Konae
// around it (ADR-0027, decision 4). Konae publishes one app, stops, and its
// own files go; the Kernel alone then serves the app's Server Actions on the
// runtime endpoint and never creates owners.sqlite or mcp.sqlite.
const kernelBinary = path.resolve(import.meta.dir, "..", "kernel-rs/target/debug/kernel");
let root: string;
let konae: Kernel | undefined;
let alone: ReturnType<typeof Bun.spawn> | undefined;

const actions = { version: 1, actions: {
  "notes.add": { kind: "mutation", access: "anonymous", statement: "INSERT INTO notes(body) VALUES(?1)", bindings: [{ source: "input", key: "body", type: "string", maxLength: 200 }] },
  "notes.list": { kind: "query", access: "anonymous", statement: "SELECT body FROM notes ORDER BY id LIMIT 50", bindings: [] },
} };
const payload = (id: string) => ({
  project: { id, name: "Alone", slug: "alone", manifest: { version: 1, visibility: "public" }, files: { "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);", "actions.json": JSON.stringify(actions) } },
  visibility: "public",
  artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Alone</main>" } },
});

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "konae-kernel-alone-"));
}, 300_000);

afterAll(async () => {
  await konae?.stop();
  if (alone && alone.exitCode === null) { alone.kill(); await alone.exited; }
  if (root) await rm(root, { recursive: true, force: true });
});

test("the kernel binary alone serves an app folder's Server Actions on the runtime endpoint", async () => {
  const port = Number(process.env.KONAE_KERNEL_ALONE_TEST_PORT) || 19800 + Math.floor(Math.random() * 90);
  const data = path.join(root, "data");
  konae = await startKernel({ port, data, setupKey: "kernel-alone-setup-secret", ownerEmail: "owner@alone.test" });
  const id = "alone_app";
  await konae.owner.call("project_create", { projectId: id, name: "Alone" });
  const { receipt } = await release(konae.owner, payload(id), id);
  await konae.stop();
  for (const file of ["owners.sqlite", "mcp.sqlite"]) await rm(path.join(data, file), { force: true });
  for (const file of ["owners.sqlite", "mcp.sqlite"]) await rm(path.join(data, `${file}-wal`), { force: true });
  for (const file of ["owners.sqlite", "mcp.sqlite"]) await rm(path.join(data, `${file}-shm`), { force: true });

  const { KONAE_ADMIN_KEY: _, ...env } = konae.env;
  alone = Bun.spawn([kernelBinary], { env, stdout: "ignore", stderr: "inherit" });
  let ready = false;
  for (let attempt = 0; attempt < 150 && !ready; attempt++) {
    try { ready = (await fetch(`${konae.url}/_konae/health`)).ok; } catch {}
    if (alone.exitCode !== null) throw Error("The kernel binary stopped during startup.");
    if (!ready) await Bun.sleep(100);
  }
  expect(ready).toBe(true);

  const invoke = async (name: string, input: object = {}) => {
    const response = await appFetch(konae!, receipt.hostname, "/_konae/mcp", {
      method: "POST",
      headers: { accept: "application/json, text/event-stream", "MCP-Protocol-Version": "2025-11-25" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "actions_invoke", arguments: { name, input } } }),
    });
    expect(response.status).toBe(200);
    const answer = await response.json() as { result: { isError: boolean; structuredContent: { result: unknown } } };
    expect(answer.result.isError).toBe(false);
    return answer.result.structuredContent.result;
  };
  expect(await invoke("notes.add", { body: "served alone" })).toMatchObject({ changes: 1 });
  expect(await invoke("notes.list")).toEqual([{ body: "served alone" }]);
  expect((await fetch(`${konae.url}/mcp`, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" })).status).not.toBe(200);
  for (const file of ["owners.sqlite", "mcp.sqlite"]) expect(existsSync(path.join(data, file))).toBe(false);
}, 120_000);
