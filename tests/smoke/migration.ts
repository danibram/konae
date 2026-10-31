import assert from "node:assert/strict";
import { mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import {
  account, appFetch, buildKernel, downloadMigration, importMigration, kernelBinary, release, runtime, startKernel,
  type Begun, type Kernel,
} from "../migration-kernel";

// The migration rehearsal of delivery J, on Kernels that share nothing but
// this script. Run with `bun run tests/smoke/migration.ts --port 19460`; it
// uses that port and the next four.
//
// Part one moves an app with accounts, rows, a file, an API key, a pending
// invitation, a suspension, a policy and a Signal from one Kernel to
// another, through a failed import and a restart, and proves what carried
// over, what did not, and that no moment had two Kernels taking writes.
// Part two restores a Kernel's copy into an empty data directory and proves
// the revocation record keeps a suspension and a revoked key in force.
const portIndex = process.argv.indexOf("--port");
const port = portIndex < 0 ? 19460 : Number(process.argv[portIndex + 1]);
if (!Number.isInteger(port) || port < 1024 || port > 65530) throw Error("Choose a port between 1024 and 65530. The rehearsal uses it and the next four.");

const root = await mkdtemp(path.join(tmpdir(), "konae-migration-rehearsal-"));
const setupKey = crypto.randomUUID() + crypto.randomUUID();
const kernels: Kernel[] = [];
const step = (text: string) => console.log(`- ${text}`);

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
    "page.like": {
      kind: "signal", access: "anonymous",
      bindings: [{ source: "input", key: "page", type: "string", maxLength: 80 }],
      signal: { name: "page.liked", properties: ["page"] },
    },
  },
};

const payload = (id: string, name: string) => ({
  project: { id, name, slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility: "public" }, files: {
    "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, user_id TEXT NOT NULL, body TEXT NOT NULL);",
    "actions.json": JSON.stringify(actions),
  } },
  visibility: "public",
  artifact: { version: 1, entrypoint: "index.html", files: { "index.html": `<!doctype html><main>${name}</main>`, "app.css": "main{font:16px system-ui}" } },
});

async function publish(kernel: Kernel, id: string, name: string) {
  await kernel.owner.call("project_create", { projectId: id, name });
  return (await release(kernel.owner, payload(id, name), id)).receipt;
}

async function kernel(index: number, name: string, env: Record<string, string> = {}, data?: string) {
  const started = await startKernel({ port: port + index, data: data ?? path.join(root, name), setupKey, ownerEmail: `owner@${name}.test`, env });
  kernels.push(started);
  return started;
}

/** Status and body of an app request; 503 with the operation is a refusal. */
async function hit(target: Kernel, hostname: string, route: string, init: RequestInit = {}) {
  const response = await appFetch(target, hostname, route, init);
  return { status: response.status, text: await response.text() };
}

async function waitForCopy(target: Kernel, after: number) {
  for (const deadline = Date.now() + 30_000; Date.now() < deadline; await Bun.sleep(200)) {
    const health = await (await fetch(`${target.url}/_konae/health`)).json() as { copies: { targets: { lastSuccessAt?: number; lastError?: string }[] } };
    const report = health.copies.targets[0];
    if (report?.lastSuccessAt && report.lastSuccessAt >= after) return;
    if (report?.lastError) throw Error(`The copy failed: ${report.lastError}`);
  }
  throw Error("No copy was taken.");
}

async function command(args: string[], env: Record<string, string>) {
  const child = Bun.spawn([kernelBinary, ...args], { env, stdout: "pipe", stderr: "pipe" });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  if (status !== 0) throw Error(`${args.join(" ")} failed: ${stderr}${stdout}`);
  return stdout;
}

try {
  await buildKernel();

  // -------------------------------------------------------------------------
  // Part one: migrate an app from the origin to the destination.
  const origin = await kernel(0, "origin");
  const destination = await kernel(1, "destination");
  const app = await publish(origin, "rehearsal_notes", "Rehearsal notes");
  step(`origin ${origin.url} publishes ${app.hostname}`);

  const anaCookie = await account(origin, app.hostname, "signup", "ana@rehearsal.test");
  const ana = runtime(origin, app.hostname, anaCookie);
  await ana.call("actions_invoke", { name: "notes.add", input: { body: "first note" } });
  await ana.call("actions_invoke", { name: "notes.add", input: { body: "second note" } });
  const upload = await ana.upload(new TextEncoder().encode("a file that moves"));
  const file = await ana.call<{ id: string }>("files_upload", { transferId: upload.transferId, name: "moved.txt", type: "text/plain" });
  await ana.cancel(upload.transferId);
  await account(origin, app.hostname, "signup", "bo@rehearsal.test");
  await runtime(origin, app.hostname).call("actions_invoke", { name: "page.like", input: { page: "home" } });
  const key = await origin.owner.call<{ keyId: string; key: string }>("api_keys_create", { deploymentId: app.id, name: "Sync", actions: ["hooks.note"], files: ["private"] });
  assert.equal((await hit(origin, app.hostname, "/hooks/note", { method: "POST", body: JSON.stringify({ body: "from the key" }), headers: { authorization: `Bearer ${key.key}` } })).status, 200);
  const users = (await origin.owner.call<{ users: { userId: string; email: string; state: string }[] }>("app_users_list", { deploymentId: app.id })).users;
  const bo = users.find((user) => user.email === "bo@rehearsal.test")!;
  await origin.owner.call("app_user_suspend", { deploymentId: app.id, userId: bo.userId });
  await origin.owner.call("signup_policy_set", { deploymentId: app.id, mode: "invitation" });
  await origin.owner.call("app_invitations_create", { deploymentId: app.id, emails: ["cy@rehearsal.test"], idempotencyKey: "rehearsal-invite-1" });
  let originLinks: string[] = [];
  for (const deadline = Date.now() + 10_000; originLinks.length === 0 && Date.now() < deadline; await Bun.sleep(100)) {
    originLinks = (await origin.mailbox()).filter((mail) => mail.purpose === "invitation").map((mail) => mail.link);
  }
  assert.equal(originLinks.length, 1, "the origin mailed the invitation");
  step("ana has two notes and a file, bo is suspended, cy has a pending invitation, the app has an API key, a Signal and Invitation only");

  const begun = await origin.owner.call<Begun>("migration_begin", { deploymentId: app.id, waitSeconds: 10 });
  assert.equal(begun.phase, "captured");
  const op = begun.operationId;
  step(`migration ${op} paused the origin and captured package ${begun.packageId}`);
  assert.equal((await hit(origin, app.hostname, "/", { headers: { accept: "text/html" } })).status, 503);
  assert.equal((await hit(origin, app.hostname, "/hooks/note", { method: "POST", body: JSON.stringify({ body: "late" }), headers: { authorization: `Bearer ${key.key}` } })).status, 503);
  await assert.rejects(ana.call("actions_invoke", { name: "notes.add", input: { body: "during the pause" } }));

  // A failed import: one component's bytes do not match the manifest.
  const captured = await downloadMigration(origin.owner, begun, await mkdtemp(path.join(root, "package-")));
  const damaged = { ...captured, files: { ...captured.files, database: path.join(root, "damaged.sqlite") } };
  const bytes = await readFile(captured.files.database);
  bytes[bytes.length - 1] ^= 0xff;
  await writeFile(damaged.files.database, bytes);
  await assert.rejects(importMigration(destination.owner, damaged, "rehearsal_notes"), /does not match|SHA-256/);
  assert.equal((await hit(origin, app.hostname, "/", { headers: { accept: "text/html" } })).status, 503, "the origin stays paused after a failed import");
  step("an import with a damaged component was refused; the origin stayed paused and the destination has no app");

  // The retry downloads the same package again.
  const again = await origin.owner.call<Begun>("migration_begin", { deploymentId: app.id });
  assert.deepEqual([again.operationId, again.packageId], [op, begun.packageId]);
  const retried = await downloadMigration(origin.owner, again, await mkdtemp(path.join(root, "package-")));
  const imported = await importMigration(destination.owner, retried, "rehearsal_notes");
  const copy = imported.receipt;
  const twice = await importMigration(destination.owner, retried, "rehearsal_notes");
  assert.equal(twice.receipt.id, copy.id, "the same package imported twice returns the first receipt");
  assert.equal((await hit(destination, copy.hostname, "/", { headers: { accept: "text/html" } })).status, 503);
  await destination.stop();
  await destination.start();
  assert.equal((await hit(destination, copy.hostname, "/", { headers: { accept: "text/html" } })).status, 503, "a restart keeps the import paused");
  // Neither installation takes writes now.
  for (const [target, hostname] of [[origin, app.hostname], [destination, copy.hostname]] as const) {
    assert.equal((await hit(target, hostname, "/_konae/auth/login", { method: "POST", body: JSON.stringify({ email: "ana@rehearsal.test", password: "migration-test-password" }) })).status, 503);
  }
  step(`the retry imported ${copy.hostname} paused on ${destination.url}; a repeat returned the same Deployment; a restart kept it paused; no Kernel takes writes`);

  const confirmed = await origin.owner.call<{ confirmation: string; retainUntil: number }>("migration_confirm", { operationId: op, newUrl: copy.url });
  await assert.rejects(origin.owner.call("migration_cancel", { operationId: op }), /two installations writing/);
  const activated = await destination.owner.call<{ invitations: { pending: { email: string }[]; resent: { ok: boolean }[] } }>("migration_activate", {
    deploymentId: copy.id, confirmation: confirmed.confirmation, resendInvitations: true,
  });
  assert.deepEqual(activated.invitations.pending.map((invitation) => invitation.email), ["cy@rehearsal.test"]);
  assert.ok(activated.invitations.resent.every((result) => result.ok), JSON.stringify(activated.invitations.resent));
  step(`confirmed at the origin (kept until ${new Date(confirmed.retainUntil).toISOString()}) and activated at the destination`);

  // What carried over.
  const movedUsers = (await destination.owner.call<{ users: { userId: string; email: string; state: string }[] }>("app_users_list", { deploymentId: copy.id })).users;
  assert.deepEqual(
    movedUsers.filter((user) => user.email !== "cy@rehearsal.test").map((user) => [user.userId, user.state]).sort(),
    users.map((user) => [user.userId, user.email === "bo@rehearsal.test" ? "suspended" : user.state]).sort(),
    "IDs and states carry over",
  );
  assert.equal((await hit(destination, copy.hostname, "/_konae/auth/login", { method: "POST", body: JSON.stringify({ email: "bo@rehearsal.test", password: "migration-test-password" }) })).status >= 400, true, "bo stays suspended");
  const anaAgain = await account(destination, copy.hostname, "login", "ana@rehearsal.test");
  const moved = runtime(destination, copy.hostname, anaAgain);
  assert.deepEqual(await moved.call("actions_invoke", { name: "notes.list", input: {} }), [{ body: "first note" }, { body: "second note" }]);
  const opened = await moved.call<{ transferId: string; size: number }>("files_read", { fileId: file.id });
  assert.equal(opened.size, "a file that moves".length);
  await moved.cancel(opened.transferId);
  assert.equal((await destination.owner.call<{ mode: string; visibility: string }>("signup_policy_read", { deploymentId: copy.id })).mode, "invitation");
  const signals = await destination.owner.call<{ signals: { name: string }[] }>("signals_read", { deploymentId: copy.id });
  assert.ok(signals.signals.some((signal) => signal.name === "page.liked"), "the Signal carried over");
  step("IDs, password, rows, file, suspension, Invitation only and the Signal carried over; ana signed in again with her password");

  // What did not.
  assert.equal((await runtime(destination, copy.hostname, anaCookie).call<{ user: unknown }>("auth_session")).user ?? null, null, "the origin's session does not open the copy");
  assert.equal((await hit(destination, copy.hostname, "/hooks/note", { method: "POST", body: JSON.stringify({ body: "old key" }), headers: { authorization: `Bearer ${key.key}` } })).status, 401, "the origin's API key fails");
  const oldToken = new URL(originLinks[0]).hash.slice("#invitation=".length);
  assert.notEqual((await hit(destination, copy.hostname, "/_konae/auth/accept", { method: "POST", body: JSON.stringify({ token: oldToken, password: "a password for cy" }) })).status, 200, "the origin's invitation link fails");
  let newLinks: string[] = [];
  for (const deadline = Date.now() + 10_000; newLinks.length === 0 && Date.now() < deadline; await Bun.sleep(100)) {
    newLinks = (await destination.mailbox()).filter((mail) => mail.purpose === "invitation").map((mail) => mail.link);
  }
  assert.ok(newLinks[0]?.startsWith(copy.url), `the resent invitation links to the new host: ${newLinks[0]}`);
  const newToken = new URL(newLinks[0]).hash.slice("#invitation=".length);
  assert.equal((await hit(destination, copy.hostname, "/_konae/auth/accept", { method: "POST", body: JSON.stringify({ token: newToken, password: "a password for cy" }) })).status, 200, "the resent link works");
  const retiredPage = await hit(origin, app.hostname, "/", { headers: { accept: "text/html" } });
  assert.equal(retiredPage.status, 503);
  assert.ok(retiredPage.text.includes(`href="${copy.url}/"`) || retiredPage.text.includes(`href="${copy.url}"`), "the origin's page links to the new address");
  assert.equal((await hit(origin, app.hostname, "/_konae/auth/login", { method: "POST", body: "{}" })).status, 503, "the origin refuses writes and sign-in, without a redirect");
  step("old sessions, the old API key and the old invitation link fail; the resent link works on the new host; the origin shows the new address and refuses writes");

  // Cancel before confirm resumes the origin.
  const other = await publish(origin, "rehearsal_cancel", "Cancelled move");
  const otherBegun = await origin.owner.call<Begun>("migration_begin", { deploymentId: other.id });
  for (const transfer of Object.values(otherBegun.downloads ?? {})) await origin.owner.cancel(transfer.transferId);
  await origin.owner.call("migration_cancel", { operationId: otherBegun.operationId });
  await account(origin, other.hostname, "signup", "dee@rehearsal.test");
  step("a migration cancelled before confirmation resumed its origin, which took a signup");

  // -------------------------------------------------------------------------
  // Part two: restore a copy and keep the revocation record.
  const copies = path.join(root, "copies");
  const source = await kernel(2, "restore-source", { KONAE_COPY_DIR: copies });
  const kept = await publish(source, "rehearsal_restore", "Restored app");
  await account(source, kept.hostname, "signup", "eve@rehearsal.test");
  await account(source, kept.hostname, "signup", "fay@rehearsal.test");
  const restoredKey = await source.owner.call<{ keyId: string; key: string }>("api_keys_create", { deploymentId: kept.id, name: "Kept", actions: ["hooks.note"] });
  const restartedAt = Date.now();
  await source.stop();
  await source.start();
  await waitForCopy(source, restartedAt);
  const eve = (await source.owner.call<{ users: { userId: string; email: string }[] }>("app_users_list", { deploymentId: kept.id })).users.find((user) => user.email === "eve@rehearsal.test")!;
  await source.owner.call("app_user_suspend", { deploymentId: kept.id, userId: eve.userId });
  await source.owner.call("api_keys_revoke", { deploymentId: kept.id, keyId: restoredKey.keyId });
  assert.ok((await readdir(path.join(copies, "revocations"))).length >= 3, "the suspension and the revocation are off the volume");
  await source.stop();
  step("a copy was taken; eve was suspended and the key revoked after it");

  const restoredData = path.join(root, "restored");
  const startedRestore = Date.now();
  const output = await command(["restore", "--from", copies], { ...source.env, KONAE_DATA_DIR: restoredData });
  assert.match(output, /Applied the revocation record/);
  const restoreSeconds = (Date.now() - startedRestore) / 1000;
  const restored = await startKernel({ port: port + 3, data: restoredData, setupKey, ownerEmail: "owner@restore-source.test", signIn: true });
  kernels.push(restored);
  assert.equal((await hit(restored, kept.hostname, "/", { headers: { accept: "text/html" } })).status, 200, "the restored app serves");
  assert.equal((await hit(restored, kept.hostname, "/_konae/auth/login", { method: "POST", body: JSON.stringify({ email: "eve@rehearsal.test", password: "migration-test-password" }) })).status >= 400, true, "eve stays suspended");
  await account(restored, kept.hostname, "login", "fay@rehearsal.test");
  assert.equal((await hit(restored, kept.hostname, "/hooks/note", { method: "POST", body: JSON.stringify({ body: "revived?" }), headers: { authorization: `Bearer ${restoredKey.key}` } })).status, 401, "the revoked key stays revoked");
  step(`restored into an empty data directory in ${restoreSeconds.toFixed(1)} s; eve stays suspended, the revoked key fails, fay signs in`);

  // Without a readable record the installation stays in maintenance.
  await rm(path.join(copies, "revocations", "started.json"));
  const heldData = path.join(root, "held");
  const heldOutput = await command(["restore", "--from", copies], { ...source.env, KONAE_DATA_DIR: heldData });
  assert.match(heldOutput, /starts in maintenance/);
  const held = await startKernel({ port: port + 4, data: heldData, setupKey, ownerEmail: "owner@restore-source.test", signIn: true });
  kernels.push(held);
  const heldPage = await hit(held, kept.hostname, "/", { headers: { accept: "text/html" } });
  assert.equal(heldPage.status, 503);
  assert.match(heldPage.text, /reconcile/);
  await assert.rejects(held.owner.call("signup_policy_set", { deploymentId: kept.id, mode: "closed" }), /restored from a copy/);
  await held.stop();
  assert.match(await command(["reconcile", "--without-record"], { ...held.env }), /Reopened/);
  await held.start();
  assert.equal((await hit(held, kept.hostname, "/", { headers: { accept: "text/html" } })).status, 200);
  step("a restore without a readable record started in maintenance and reopened only after reconcile --without-record");
  console.log("Migration rehearsal passed.");
} finally {
  for (const started of kernels) await started.stop().catch(() => {});
  if (!process.argv.includes("--keep")) await rm(root, { recursive: true, force: true });
}
