import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";
import {
  account, appFetch, buildKernel, downloadMigration, importMigration, release, runtime, startKernel,
  type Begun, type Deployment, type Kernel,
} from "./migration-kernel";

// Delivery J: maintenance, migration_begin, confirm, cancel and activate on
// one Kernel, and every refusal the delivery's row in
// docs/plans/platform-launch.md lists. The two-Kernel rehearsal is
// tests/smoke/migration.ts.
let root: string;
let kernel: Kernel;
let owner: McpClient;

const actions = {
  version: 1,
  actions: {
    "notes.add": {
      kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(user_id, body) VALUES(?1, ?2)",
      bindings: [{ source: "actor", field: "id" }, { source: "input", key: "body", type: "string", maxLength: 200 }],
    },
    "notes.list": {
      kind: "query", access: "authenticated", statement: "SELECT body FROM notes WHERE user_id=?1 ORDER BY id LIMIT 50",
      bindings: [{ source: "actor", field: "id" }],
    },
    "hooks.note": {
      kind: "mutation", access: "anonymous", statement: "INSERT INTO notes(user_id, body) VALUES('hook', ?1)",
      bindings: [{ source: "input", key: "body", type: "string", maxLength: 200 }],
      http: { method: "POST", path: "/hooks/note" },
    },
  },
};

function payload(id: string) {
  return {
    project: { id, name: `Migration ${id}`, slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility: "public" }, files: {
      "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, user_id TEXT NOT NULL, body TEXT NOT NULL);",
      "actions.json": JSON.stringify(actions),
    } },
    visibility: "public",
    artifact: { version: 1, entrypoint: "index.html", files: { "index.html": `<!doctype html><main>${id} serving</main>` } },
  };
}

async function publish(id: string) {
  await owner.call("project_create", { projectId: id, name: `Migration ${id}` });
  return (await release(owner, payload(id), id)).receipt;
}

const html = { accept: "text/html" };
const page = async (app: Deployment) => {
  const response = await appFetch(kernel, app.hostname, "/", { headers: html });
  return { status: response.status, body: await response.text() };
};

async function refusedWith(response: Response, operationId: string) {
  expect(response.status).toBe(503);
  const body = await response.json() as { error: string; operationId: string };
  expect(body.operationId).toBe(operationId);
  return body.error;
}

async function begin(app: Deployment) {
  const begun = await owner.call<Begun>("migration_begin", { deploymentId: app.id, waitSeconds: 10 });
  expect(begun.phase).toBe("captured");
  return begun;
}

/** Frees the transfers of downloads a test does not read. */
async function drop(begun: Begun) {
  for (const transfer of Object.values(begun.downloads ?? {})) await owner.cancel(transfer.transferId).catch(() => {});
}

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "littlebox-migration-"));
  const port = Number(process.env.LITTLEBOX_MIGRATION_TEST_PORT) || 19700 + Math.floor(Math.random() * 90);
  kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: "migration-integration-setup-secret", ownerEmail: "owner@migration.test" });
  owner = kernel.owner;
}, 300_000);

afterAll(async () => { await kernel?.stop(); if (root) await rm(root, { recursive: true, force: true }); });

describe("migration", () => {
  test("migration_begin needs manage_lifecycle and export_data for the app's project", async () => {
    const app = await publish("mig_scopes");
    await publish("mig_scopes_other");
    const token = async (scopes: string[], projectIds = ["mig_scopes"]) => {
      const { token } = await owner.call<{ token: string }>("token_create", { scopes, projectIds, ttlSeconds: 600 });
      return new McpClient(`${kernel.url}/mcp`, { token });
    };
    await expect((await token(["read", "manage_lifecycle"])).call("migration_begin", { deploymentId: app.id })).rejects.toThrow("Permission denied for export_data.");
    await expect((await token(["read", "export_data", "write", "publish"])).call("migration_begin", { deploymentId: app.id })).rejects.toThrow("Unknown tool or insufficient permission.");
    await expect((await token(["read", "export_data", "manage_lifecycle"], ["mig_scopes_other"])).call("migration_begin", { deploymentId: app.id })).rejects.toThrow("outside this token's grants");
    await expect((await token(["read"])).call("migration_status", { deploymentId: app.id })).resolves.toMatchObject({ phase: "none", paused: false });
    expect((await page(app)).status).toBe(200);
  });

  test("a paused app refuses writes, signups, recovery, administrative changes and email, also after a restart, until it is cancelled", async () => {
    const app = await publish("mig_pause");
    const cookie = await account(kernel, app.hostname, "signup", "ana@pause.test");
    const writer = runtime(kernel, app.hostname, cookie);
    await writer.call("actions_invoke", { name: "notes.add", input: { body: "before the pause" } });
    const users = await owner.call<{ users: { userId: string }[] }>("app_users_list", { deploymentId: app.id });
    const userId = users.users[0].userId;

    const begun = await begin(app);
    expect(begun.paused).toBe(true);
    expect(begun.manifest?.version).toBe(2);
    expect(begun.manifest?.migration?.operationId).toBe(begun.operationId);
    expect(Object.keys(begun.downloads ?? {}).sort()).toEqual(["database", "files", "identity", "release", "usage"]);
    const op = begun.operationId;
    await drop(begun);

    const shell = await page(app);
    expect(shell.status).toBe(503);
    expect(shell.body).toContain("is in maintenance");
    expect(shell.body).toContain(op);
    // Every write path answers with the operation, and nothing is redirected.
    const mcp = (name: string, args: object) => appFetch(kernel, app.hostname, "/_littlebox/mcp", {
      method: "POST", headers: { cookie }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } }),
    });
    expect(await refusedWith(await mcp("actions_invoke", { name: "notes.add", input: { body: "during the pause" } }), op)).toContain(`paused for migration ${op}`);
    await refusedWith(await mcp("actions_invoke", { name: "notes.list", input: {} }), op);
    for (const [route, body] of [
      ["/_littlebox/auth/signup", { email: "new@pause.test", password: "a long enough password" }],
      ["/_littlebox/auth/login", { email: "ana@pause.test", password: "migration-test-password" }],
      ["/_littlebox/auth/recover", { email: "ana@pause.test" }],
      ["/_littlebox/auth/accept", { token: "x", password: "a long enough password" }],
      ["/hooks/note", { body: "from a webhook" }],
    ] as const) {
      await refusedWith(await appFetch(kernel, app.hostname, route, { method: "POST", body: JSON.stringify(body) }), op);
    }
    await refusedWith(await appFetch(kernel, app.hostname, `/_littlebox/files/${crypto.randomUUID()}`), op);

    const paused = `paused for migration ${op}`;
    await expect(owner.call("signup_policy_set", { deploymentId: app.id, mode: "closed" })).rejects.toThrow(paused);
    await expect(owner.call("app_user_suspend", { deploymentId: app.id, userId })).rejects.toThrow(paused);
    await expect(owner.call("app_invitations_create", { deploymentId: app.id, emails: ["bo@pause.test"], idempotencyKey: "pause-invite-1" })).rejects.toThrow(paused);
    await expect(owner.call("api_keys_create", { deploymentId: app.id, name: "Paused key", actions: ["hooks.note"] })).rejects.toThrow(paused);
    await expect(owner.call("secret_set", { projectId: "mig_pause", name: "orders_webhook", value: "a-long-enough-secret" })).rejects.toThrow(paused);
    await expect(owner.call("database_execute", { deploymentId: app.id, operation: "sql", input: { sql: "DELETE FROM notes" } })).rejects.toThrow(paused);
    await expect(release(owner, payload("mig_pause"), "mig_pause")).rejects.toThrow(paused);
    // Reads stay open.
    expect((await owner.call<{ users: unknown[] }>("app_users_list", { deploymentId: app.id })).users).toHaveLength(1);
    expect(await owner.call("database_execute", { deploymentId: app.id, operation: "inspect" })).toBeTruthy();

    // A restart keeps the pause.
    await kernel.stop();
    await kernel.start();
    expect((await page(app)).status).toBe(503);
    expect(await owner.call("migration_status", { deploymentId: app.id })).toMatchObject({ operationId: op, phase: "captured", paused: true });
    // A second begin continues the same operation with the same package.
    const again = await owner.call<Begun>("migration_begin", { deploymentId: app.id });
    expect([again.operationId, again.packageId]).toEqual([op, begun.packageId]);
    await drop(again);

    const cancelled = await owner.call<{ operationId: string; cancelledAt: number }>("migration_cancel", { operationId: op });
    expect(cancelled.operationId).toBe(op);
    expect(await owner.call("migration_cancel", { operationId: op })).toEqual(cancelled);
    expect((await page(app)).status).toBe(200);
    await writer.call("actions_invoke", { name: "notes.add", input: { body: "after the cancel" } });
    expect(await writer.call<unknown[]>("actions_invoke", { name: "notes.list", input: {} })).toHaveLength(2);
    await account(kernel, app.hostname, "signup", "new@pause.test");
    await expect(owner.call("migration_confirm", { operationId: op })).rejects.toThrow("was cancelled");
  }, 120_000);

  test("an import starts paused, activates only with the origin's confirmation, and repeats return the first receipt", async () => {
    const app = await publish("mig_move");
    const cookie = await account(kernel, app.hostname, "signup", "ana@move.test");
    await runtime(kernel, app.hostname, cookie).call("actions_invoke", { name: "notes.add", input: { body: "moved row" } });
    const begun = await begin(app);
    const captured = await downloadMigration(owner, begun, await mkdtemp(path.join(root, "move-")));

    const imported = await importMigration(owner, captured, "mig_move_copy");
    const copy = imported.receipt;
    expect(copy.id).not.toBe(app.id);
    const shell = await page(copy);
    expect(shell.status).toBe(503);
    expect(shell.body).toContain("migration_activate");
    await refusedWith(await appFetch(kernel, copy.hostname, "/_littlebox/auth/login", { method: "POST", body: JSON.stringify({ email: "ana@move.test", password: "migration-test-password" }) }), begun.operationId);
    expect(await owner.call("migration_status", { deploymentId: copy.id })).toMatchObject({ role: "destination", phase: "imported", paused: true });
    await expect(owner.call("migration_begin", { deploymentId: copy.id })).rejects.toThrow("not active yet");
    // Before the origin confirms there is no confirmation to give.
    await expect(owner.call("migration_activate", { deploymentId: copy.id, confirmation: "0".repeat(64) })).rejects.toThrow("does not belong to migration");
    await expect(owner.call("migration_activate", { deploymentId: app.id, confirmation: "0".repeat(64) })).rejects.toThrow("This one is the origin of migration");

    // The same package again answers with the first import.
    const replay = await importMigration(owner, captured, "mig_move_copy");
    expect(replay.plan).toMatchObject({ planId: imported.plan.planId, applied: true });
    expect(replay.receipt.id).toBe(copy.id);
    // Another package into the same, non-empty app is refused.
    const other = await begin(await publish("mig_other"));
    const otherCaptured = await downloadMigration(owner, other, await mkdtemp(path.join(root, "other-")));
    await expect(importMigration(owner, otherCaptured, "mig_move_copy")).rejects.toThrow("first deployment");
    await owner.call("migration_cancel", { operationId: other.operationId });

    await expect(owner.call("migration_confirm", { operationId: begun.operationId, newUrl: "http://notes.example.org" })).rejects.toThrow("must use https");
    const confirmed = await owner.call<{ confirmation: string; retainUntil: number; confirmedAt: number }>("migration_confirm", { operationId: begun.operationId, newUrl: "https://notes.example.org" });
    expect(confirmed.retainUntil - confirmed.confirmedAt).toBe(7 * 24 * 60 * 60 * 1000);
    expect(await owner.call("migration_confirm", { operationId: begun.operationId })).toEqual(confirmed);
    expect(JSON.stringify(await owner.call("migration_status", { deploymentId: app.id }))).not.toContain(confirmed.confirmation);
    await expect(owner.call("migration_cancel", { operationId: begun.operationId })).rejects.toThrow("two installations writing");
    await expect(owner.call("migration_begin", { deploymentId: app.id })).rejects.toThrow("was confirmed");
    const retired = await page(app);
    expect(retired.status).toBe(503);
    expect(retired.body).toContain('href="https://notes.example.org/"');
    expect(retired.body).toContain("moved to another server");
    // POST and auth routes are refused, never redirected.
    expect((await appFetch(kernel, app.hostname, "/_littlebox/auth/login", { method: "POST", body: "{}" })).status).toBe(503);

    await expect(owner.call("migration_activate", { deploymentId: copy.id, confirmation: confirmed.confirmation.replace(/.$/, "0") })).rejects.toThrow("does not belong");
    const activated = await owner.call<{ operationId: string; url: string }>("migration_activate", { deploymentId: copy.id, confirmation: confirmed.confirmation });
    expect(activated.operationId).toBe(begun.operationId);
    expect(await owner.call("migration_activate", { deploymentId: copy.id, confirmation: confirmed.confirmation })).toEqual(activated);
    expect((await page(copy)).status).toBe(200);
    const signedIn = await account(kernel, copy.hostname, "login", "ana@move.test");
    expect(await runtime(kernel, copy.hostname, signedIn).call("actions_invoke", { name: "notes.list", input: {} })).toEqual([{ body: "moved row" }]);
    // The origin's session does not open the copy.
    expect((await runtime(kernel, copy.hostname, cookie).call<{ user: unknown }>("auth_session")).user ?? null).toBeNull();
  }, 120_000);

  test("an origin whose kept package is gone, as after a restore, captures again under the same operation", async () => {
    const app = await publish("mig_recapture");
    const begun = await begin(app);
    await drop(begun);
    await rm(path.join(kernel.data, "deployments", app.id, "migration", begun.operationId), { recursive: true });
    const again = await owner.call<Begun>("migration_begin", { deploymentId: app.id });
    expect(again.phase).toBe("captured");
    expect(again.operationId).toBe(begun.operationId);
    expect(again.packageId).not.toBe(begun.packageId);
    expect(again.manifest?.migration?.confirmationHash).toBe(begun.manifest?.migration?.confirmationHash);
    await drop(again);
    await owner.call("migration_cancel", { operationId: begun.operationId });
  }, 60_000);

  test("cancelling at the origin leaves an imported copy unable to activate", async () => {
    const app = await publish("mig_cancel");
    const begun = await begin(app);
    const captured = await downloadMigration(owner, begun, await mkdtemp(path.join(root, "cancel-")));
    const copy = (await importMigration(owner, captured, "mig_cancel_copy")).receipt;
    await owner.call("migration_cancel", { operationId: begun.operationId });
    expect((await page(app)).status).toBe(200);
    expect((await page(copy)).status).toBe(503);
    await expect(owner.call("migration_confirm", { operationId: begun.operationId })).rejects.toThrow("was cancelled");
    await expect(owner.call("migration_activate", { deploymentId: copy.id, confirmation: "f".repeat(64) })).rejects.toThrow("does not belong");
  }, 120_000);
});
