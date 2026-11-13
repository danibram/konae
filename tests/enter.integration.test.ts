import { afterAll, beforeAll, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { account, appFetch, buildKernel, release, runtime, startKernel, type Kernel } from "./kernel-harness";
import type { UsageSummary } from "../src/lib/usage";

// An owner enters an app as one of its users through a one-time link
// (kernel-rs/kernel/src/enter.rs). The owner's audit log records who entered
// as whom, and Usage and Signals do not count what that session does.
let root: string;
let kernel: Kernel;

const actions = { version: 1, actions: {
  "notes.add": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(body, owner_id) VALUES(?1, ?2)",
    bindings: [{ source: "input", key: "body", type: "string", maxLength: 200 }, { source: "actor", field: "id" }],
    signal: { name: "note.added", properties: ["actor"] } },
  "notes.mine": { kind: "query", access: "authenticated", statement: "SELECT body FROM notes WHERE owner_id = ?1 ORDER BY id LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
} };
const payload = (id: string) => ({
  project: { id, name: "Entered", slug: "entered", manifest: { version: 1, visibility: "public" }, files: { "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL, owner_id TEXT NOT NULL);", "actions.json": JSON.stringify(actions) } },
  visibility: "public",
  artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Entered app</main>" } },
});

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "konae-enter-"));
  const port = Number(process.env.KONAE_ENTER_TEST_PORT) || 19800 + Math.floor(Math.random() * 90);
  kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: "enter-setup-secret", ownerEmail: "owner@enter.test" });
}, 300_000);

afterAll(async () => { await kernel?.stop(); if (root) await rm(root, { recursive: true, force: true }); });

test("an owner enters as an app user once per link, and the audit log names both while Usage and Signals skip the session", async () => {
  const id = "enter_app";
  await kernel.owner.call("project_create", { projectId: id, name: "Entered" });
  const { receipt } = await release(kernel.owner, payload(id), id);
  const cookie = await account(kernel, receipt.hostname, "signup", "ana@enter.test");
  const ana = (await kernel.owner.call<{ users: { userId: string; email: string }[] }>("app_users_list", { deploymentId: receipt.id })).users[0];
  await runtime(kernel, receipt.hostname, cookie).call("auth_session");
  await runtime(kernel, receipt.hostname, cookie).call("actions_invoke", { name: "notes.add", input: { body: "Ana's own note" } });

  const link = await kernel.owner.call<{ userId: string; email: string; url: string; expiresAt: number }>("app_user_enter", { deploymentId: receipt.id, userId: ana.userId });
  expect(link).toMatchObject({ userId: ana.userId, email: "ana@enter.test" });
  expect(link.url).toStartWith(`${receipt.url}/_konae/enter#entry=`);
  const token = new URL(link.url).hash.replace("#entry=", "");

  // The page does nothing on GET; the POST from the app's own origin takes the token.
  expect((await appFetch(kernel, receipt.hostname, "/_konae/enter")).status).toBe(200);
  const enter = () => appFetch(kernel, receipt.hostname, "/_konae/enter", { method: "POST", body: JSON.stringify({ token }), headers: { origin: `http://${receipt.hostname}` } });
  const entered = await enter();
  expect(entered.status).toBe(200);
  const enteredCookie = entered.headers.get("set-cookie")!.split(";")[0];
  expect((await enter()).status).toBe(410);

  const as = runtime(kernel, receipt.hostname, enteredCookie);
  expect(await as.call("auth_session", {})).toMatchObject({ userId: ana.userId, name: "ana@enter.test", role: "member" });
  await as.call("actions_invoke", { name: "notes.add", input: { body: "Written by the owner as Ana" } });
  expect(await as.call<{ body: string }[]>("actions_invoke", { name: "notes.mine", input: {} })).toHaveLength(2);

  const today = new Date().toISOString().slice(0, 10);
  const usage = await kernel.owner.call<{ totals: { kind: string; name: string; count: number }[] }>("usage_read", { deploymentId: receipt.id, from: today, to: today });
  const total = (kind: string, name: string) => usage.totals.find((row) => row.kind === kind && row.name === name)?.count ?? 0;
  expect(total("action", "notes.add")).toBe(1);
  expect(total("action", "notes.mine")).toBe(0);
  expect(total("login", "")).toBe(0);
  const summary = await kernel.owner.call<UsageSummary>("usage_summary", { deploymentId: receipt.id, from: today, to: today });
  expect(summary.accounts).toMatchObject({ active: 1, opens: 1, actions: 1, top: [
    { account: ana.userId, email: ana.email, opens: 1, actions: 1, activeDays: 1 },
  ] });
  const signals = await kernel.owner.call<{ signals: { properties: Record<string, unknown> }[] }>("signals_read", { deploymentId: receipt.id, name: "note.added" });
  expect(signals.signals.map((signal) => signal.properties)).toEqual([{ actor: ana.userId }]);

  const audit = new Database(path.join(kernel.data, "mcp.sqlite"));
  try {
    const row = audit.query("SELECT actor, target, detail, ok FROM audit WHERE tool = 'app_user_enter'").get() as { actor: string; target: string; detail: string; ok: number };
    expect(row).toMatchObject({ target: receipt.id, detail: `as ana@enter.test (${ana.userId})`, ok: 1 });
    expect(row.actor).toStartWith("owner:");
  } finally { audit.close(); }

  // A suspended account cannot be entered.
  await kernel.owner.call("app_user_suspend", { deploymentId: receipt.id, userId: ana.userId });
  await expect(kernel.owner.call("app_user_enter", { deploymentId: receipt.id, userId: ana.userId })).rejects.toThrow("Only an active account can be entered.");
}, 120_000);
