// Replays the inbox showcase against its own Kernel: publish, the signed webhook,
// the keyed read route, invitations and suspension, Signals and Usage, the LIMIT
// refusal in the Gate and the Kernel, the Action Contract, and a browser pass.
//
//   LITTLEBOX_SHOWCASE_PORT=19470 bun run examples/showcase/inbox/replay.ts
//
// LITTLEBOX_SHOWCASE_BROWSER=0 skips Chrome. LITTLEBOX_SHOWCASE_SCREENSHOTS=<dir>
// keeps the light and dark screenshots there.
import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createHmac } from "node:crypto";
import initSqlJs from "sql.js";
import { McpClient, McpError } from "../../../src/mcp/client";
import { buildLocal } from "../../../scripts/littlebox";
import { createCapsuleValidator } from "../../../src/agent/gate";
import { starterProject } from "../../../src/lib/starter";
import type { CapsuleFiles } from "../../../src/agent/contracts";

const repo = path.resolve(import.meta.dir, "../../..");
const source = import.meta.dir;
const scratch = await mkdtemp(path.join(tmpdir(), "littlebox-showcase-inbox-"));
const dataDir = path.join(scratch, "data");
const port = Number(process.env.LITTLEBOX_SHOWCASE_PORT ?? 19470);
assert(Number.isInteger(port) && port >= 1024 && port <= 65535, "Set LITTLEBOX_SHOWCASE_PORT to a free local port.");
const url = `http://127.0.0.1:${port}`;
const setupKey = crypto.randomUUID();
const password = crypto.randomUUID();
const webhookSecret = `whk_${crypto.randomUUID().replaceAll("-", "")}`;
const feedSecret = `feed_${crypto.randomUUID().replaceAll("-", "")}`;
const today = new Date().toISOString().slice(0, 10);

let kernel: ReturnType<typeof Bun.spawn> | undefined;
let address = 0;
const freshAddress = () => `198.18.${Math.floor(++address / 250)}.${(address % 250) + 1}`;
const passes: string[] = [];
const refusals: { what: string; status?: number; message: string }[] = [];
function pass(scenario: string, detail: string) {
  passes.push(scenario);
  console.log(`PASS ${scenario}. ${detail}`);
}
function refused(what: string, message: string, status?: number) {
  refusals.push({ what, status, message });
}
const messageOf = (error: unknown) => error instanceof Error ? error.message : String(error);
async function rejects(promise: Promise<unknown>, pattern: RegExp | string): Promise<string> {
  try {
    await promise;
  } catch (error) {
    const message = messageOf(error);
    if (typeof pattern === "string") assert.equal(message, pattern);
    else assert.match(message, pattern);
    return message;
  }
  assert.fail(`Expected a refusal matching ${pattern}.`);
}

try {
  // ---------------------------------------------------------------- Kernel
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
  assert.equal(await build.exited, 0, "The Kernel must compile.");
  kernel = Bun.spawn([path.join(repo, "kernel-rs/target/debug/littlebox-kernel")], {
    cwd: repo,
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_") && !key.startsWith("VITE_"))),
      LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_PUBLIC_ORIGIN: url,
      LITTLEBOX_HOST_SUFFIX: "localhost", LITTLEBOX_DATA_DIR: dataDir,
      LITTLEBOX_ADMIN_KEY: setupKey, LITTLEBOX_BUN_PATH: process.execPath, LITTLEBOX_SOURCE_ROOT: repo,
      LITTLEBOX_TRUSTED_PROXIES: "127.0.0.1", LITTLEBOX_MAIL_TRANSPORT: "development",
    },
    stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 150 && !ready; attempt++) {
    assert.equal(kernel.exitCode, null, "The Kernel stopped before startup. Is the port taken?");
    try { ready = (await fetch(`${url}/_littlebox/health`)).ok; } catch {}
    if (!ready) await Bun.sleep(100);
  }
  assert(ready, "The Kernel did not start.");

  const ownerSignup = await fetch(`${url}/_littlebox/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@inbox.example", password, setupKey }),
  });
  assert.equal(ownerSignup.status, 200, await ownerSignup.clone().text());
  const ownerCookie = ownerSignup.headers.get("set-cookie")!.split(";")[0];
  const owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", ownerCookie);
    return fetch(input, { ...init, headers });
  } });

  // --------------------------------------------------------------- Publish
  const root = path.join(scratch, "inbox");
  await cp(source, root, { recursive: true, filter: (from) => !from.endsWith("replay.ts") && !from.endsWith("README.md") });
  const project = { id: "showcase_inbox", name: "Inbox", slug: "inbox", manifest: { version: 1, visibility: "private" } };
  await owner.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });

  async function release(target: typeof project, files: Record<string, string>, artifact: unknown, extra: object = {}) {
    const payload = { project: { ...target, files }, visibility: target.manifest.visibility, artifact, ...extra };
    const upload = await owner.upload(new TextEncoder().encode(JSON.stringify(payload)), target.id);
    try {
      const plan = await owner.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
      return await owner.call<{ id: string; hostname: string; url: string }>("release_apply", { planId: plan.planId });
    } finally { await owner.cancel(upload.transferId); }
  }

  const compiled = await buildLocal(root);
  // One refusal names both unset secrets, so one failed plan tells the owner everything to set.
  const unsetSecret = await rejects(release(project, compiled.files, compiled.artifact), /which are not set for this project/);
  assert.equal(unsetSecret, "Actions events.feed and events.receive sign with the secrets events_feed and events_webhook, which are not set for this project. Set each with secret_set before publishing.");
  refused("Publish before secret_set", unsetSecret);
  await owner.call("secret_set", { projectId: project.id, name: "events_webhook", value: webhookSecret });
  await owner.call("secret_set", { projectId: project.id, name: "events_feed", value: feedSecret });
  const listedSecrets = await owner.call<{ secrets: Array<{ name: string; value?: string }> }>("secrets_list", { projectId: project.id });
  assert.deepEqual(listedSecrets.secrets.map((secret) => secret.name).sort(), ["events_feed", "events_webhook"]);
  assert(!JSON.stringify(listedSecrets).includes(webhookSecret), "secrets_list must not return a value.");
  const inbox = await release(project, compiled.files, compiled.artifact, { initialSignupPolicy: { mode: "invitation", allowlist: [] } });
  const policy = await owner.call<{ mode: string; visibility: string }>("signup_policy_read", { deploymentId: inbox.id });
  assert.equal(policy.mode, "invitation");
  assert.equal(policy.visibility, "private");
  pass("Publish", `The release with both secrets unset is refused with one message naming both; after secret_set it publishes private with signup by invitation at ${inbox.url}.`);

  const host = (extra: Record<string, string> = {}) => ({ host: inbox.hostname, "x-forwarded-for": freshAddress(), ...extra });
  const runtime = (cookie?: string) => new McpClient(`${url}/_littlebox/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", inbox.hostname); headers.set("x-forwarded-for", freshAddress());
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  const action = <T = any>(client: McpClient, name: string, input: object = {}) => client.call<T>("actions_invoke", { name, input });

  // ------------------------------------------- Private shell, signed route
  const shell = await fetch(`${url}/`, { headers: host(), redirect: "manual" });
  assert.match(await shell.text(), /<title>Sign in<\/title>/);
  const visitorCall = await rejects(action(runtime(), "events.list"), /Sign in/);
  refused("Signed-out visitor calls events.list on the runtime endpoint", visitorCall);
  const strangerSignup = await fetch(`${url}/_littlebox/auth/signup`, {
    method: "POST", headers: host({ "content-type": "application/json" }), body: JSON.stringify({ email: "stranger@elsewhere.example", password }),
  });
  assert.equal(strangerSignup.status, 403);
  refused("A stranger signs up on an Invitation only app", (await strangerSignup.json()).error, 403);

  const sign = (body: string, secret = webhookSecret, at = Math.floor(Date.now() / 1000)) =>
    `t=${at},v1=${createHmac("sha256", secret).update(`${at}.${body}`).digest("hex")}`;
  const post = async (body: string, signature?: string, extra: Record<string, string> = {}) => {
    const headers: Record<string, string> = host({ "content-type": "application/json", ...extra });
    if (signature) headers["x-inbox-signature"] = signature;
    const response = await fetch(`${url}/hooks/events`, { method: "POST", headers, body });
    return { status: response.status, body: await response.json() as any };
  };
  let delivered = 0;
  const deliver = async (event: object) => {
    const body = JSON.stringify(event);
    const answer = await post(body, sign(body));
    assert.equal(answer.status, 200, JSON.stringify(answer.body));
    delivered++;
    return answer.body.result as { changes: number; lastInsertId: number };
  };

  const first = JSON.stringify({ source: "billing", id: "inv-4471", severity: "critical", title: "Card processor declines every charge", body: "Gateway answers 502 since 09:12 UTC." });
  const firstSignature = sign(first);
  const accepted = await post(first, firstSignature);
  assert.deepEqual(accepted, { status: 200, body: { result: { changes: 1, lastInsertId: 1 } } });
  delivered++;
  const replayed = await post(first, firstSignature);
  assert.deepEqual(replayed, { status: 409, body: { error: "replayed-signature" } });
  refused("The same signed request sent twice", replayed.body.error, replayed.status);
  const wrongKey = await post(first, sign(first, "not-the-inbox-secret-at-all"));
  assert.deepEqual(wrongKey, { status: 401, body: { error: "invalid-signature" } });
  refused("A request signed with another secret", wrongKey.body.error, wrongKey.status);
  const tampered = await post(first.replace("critical", "info"), sign(first));
  assert.deepEqual(tampered, { status: 401, body: { error: "invalid-signature" } });
  refused("A body changed after signing", tampered.body.error, tampered.status);
  const stale = await post(first, sign(first, webhookSecret, Math.floor(Date.now() / 1000) - 400));
  assert.deepEqual(stale, { status: 401, body: { error: "invalid-signature" } });
  refused("A correct signature with t 400 seconds old", stale.body.error, stale.status);
  const future = await post(first, sign(first, webhookSecret, Math.floor(Date.now() / 1000) + 400));
  assert.deepEqual(future, { status: 401, body: { error: "invalid-signature" } });
  const unsigned = await post(first);
  assert.deepEqual(unsigned, { status: 401, body: { error: "invalid-signature" } });
  refused("An unsigned request", unsigned.body.error, unsigned.status);

  // A retry is the same body and, inside the same second, the same signature, so it
  // reads as a replay. A sender that signs its retry with a later t gets 200 and
  // the ON CONFLICT clause stores nothing new.
  const sameSecond = await post(first, sign(first, webhookSecret, Number(firstSignature.slice(2, firstSignature.indexOf(",")))));
  assert.deepEqual(sameSecond, { status: 409, body: { error: "replayed-signature" } });
  const retry = await post(first, sign(first, webhookSecret, Math.floor(Date.now() / 1000) - 5));
  assert.deepEqual(retry, { status: 200, body: { result: { changes: 0, lastInsertId: 0 } } });
  delivered++;
  const badSeverity = JSON.stringify({ source: "billing", id: "inv-4472", severity: "urgent", title: "Unknown severity" });
  const checkFailed = await post(badSeverity, sign(badSeverity));
  assert.equal(checkFailed.status, 400);
  assert.match(checkFailed.body.error, /CHECK constraint failed/);
  refused("A severity outside the CHECK constraint", checkFailed.body.error, 400);
  const extraField = JSON.stringify({ source: "billing", id: "inv-4473", severity: "info", title: "Extra field", priority: 2 });
  const undeclared = await post(extraField, sign(extraField));
  assert.deepEqual(undeclared, { status: 400, body: { error: "Unexpected action input: priority" } });
  refused("A sender payload with a field the action does not declare", undeclared.body.error, 400);
  // A route belongs to the Deployment; the editor's own login cookie does not replace a signature.
  const ownerCookieOnRoute = await post(first, undefined, { cookie: ownerCookie });
  assert.equal(ownerCookieOnRoute.status, 401);

  await deliver({ source: "uptime", id: "chk-api-eu", severity: "warning", title: "api.eu answers in 2.4 s", body: "p95 above 2 s for 10 minutes." });
  await deliver({ source: "deploys", id: "rel-812", severity: "info", title: "Release 812 reached production" });
  await deliver({ source: "uptime", id: "chk-web", severity: "critical", title: "www returns 503 in two regions", body: "Failing checks from Frankfurt and Virginia." });
  await deliver({ source: "backups", id: "nightly-1106", severity: "info", title: "Nightly copy finished", body: "1.8 GiB in 4 min." });
  pass("Signed webhook on a private app", `With no session the shell answers the sign-in page, yet POST /hooks/events stores ${delivered - 1} events (one retry stored nothing); replayed, wrong-secret, tampered, stale (t-400 s), future and unsigned requests are refused.`);

  // ------------------------------------------------------- Keyed read route
  const feed = async (query = "", authorization?: string) => {
    const headers: Record<string, string> = host();
    if (authorization) headers.authorization = `Bearer ${authorization}`;
    const response = await fetch(`${url}/api/events${query}`, { headers });
    return { status: response.status, body: await response.json() as any };
  };
  const anonymousFeed = await feed();
  assert.deepEqual(anonymousFeed, { status: 401, body: { error: "invalid-signature" } });
  refused("GET /api/events with no key", anonymousFeed.body.error, 401);
  const emptyKey = await rejects(owner.call("api_keys_create", { deploymentId: inbox.id, name: "Nothing", actions: [], files: [] }), /Name at least one/);
  refused("api_keys_create naming no action and no file visibility", emptyKey);
  const unroutedKey = await rejects(owner.call("api_keys_create", { deploymentId: inbox.id, name: "In-app list", actions: ["events.list"] }), /.+/);
  refused("api_keys_create naming an authenticated action with no route", unroutedKey);
  const reader = await owner.call<{ keyId: string; key: string }>("api_keys_create", { deploymentId: inbox.id, name: "Pager mirror", actions: ["events.feed"] });
  const filesOnly = await owner.call<{ keyId: string; key: string }>("api_keys_create", { deploymentId: inbox.id, name: "Status page files", files: ["public"] });
  assert.match(reader.key, /^lbk_/);
  const read = await feed("", reader.key);
  assert.equal(read.status, 200);
  assert.equal(read.body.result.length, 5);
  assert.deepEqual(Object.keys(read.body.result[0]).sort(), ["acknowledged_at", "external_id", "id", "received_at", "severity", "source", "title"]);
  const id: Record<string, number> = Object.fromEntries(read.body.result.map((row: any) => [row.external_id, row.id]));
  const after = await feed(`?after=${id["rel-812"]}`, reader.key);
  assert.deepEqual(after.body.result.map((row: any) => row.external_id), ["chk-web", "nightly-1106"]);
  const wrongType = await feed("?after=three", reader.key);
  assert.equal(wrongType.status, 400);
  refused("GET /api/events?after=three", wrongType.body.error, 400);
  const keyOnHook = await post(first, undefined, { authorization: `Bearer ${reader.key}` });
  assert.deepEqual(keyOnHook, { status: 403, body: { error: "This API key may not call events.receive." } });
  const noAction = await feed("", filesOnly.key);
  assert.deepEqual(noAction, { status: 403, body: { error: "This API key may not call events.feed." } });
  refused("A key that names only file visibilities reads GET /api/events", noAction.body.error, 403);
  const keyOnRuntime = await fetch(`${url}/_littlebox/mcp`, {
    method: "POST", headers: host({ "content-type": "application/json", accept: "application/json, text/event-stream", authorization: `Bearer ${reader.key}` }),
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "actions_invoke", arguments: { name: "events.list", input: {} } } }),
  });
  assert.notEqual(keyOnRuntime.status, 200);
  refused("An API key on the runtime MCP endpoint", `${keyOnRuntime.status} ${(await keyOnRuntime.text()).slice(0, 160)}`, keyOnRuntime.status);
  await owner.call("api_keys_revoke", { deploymentId: inbox.id, keyId: reader.keyId });
  const revoked = await feed("", reader.key);
  assert.deepEqual(revoked, { status: 401, body: { error: "invalid-api-key" } });
  refused("The revoked key reads GET /api/events", revoked.body.error, 401);
  const keys = await owner.call<{ keys: Array<{ keyId: string; revokedAt?: number | null }> }>("api_keys_list", { deploymentId: inbox.id });
  assert(keys.keys.find((key) => key.keyId === reader.keyId)?.revokedAt, "api_keys_list shows the revocation.");
  assert(!JSON.stringify(keys).includes(reader.key));
  const routeReads = 2;
  pass("Keyed read route", "A key naming events.feed reads 5 rows and filters with ?after; a key naming nothing cannot be created; a files-only key gets 403; the runtime endpoint ignores keys; after revocation the key gets 401 invalid-api-key.");

  // A private app's unsigned GET route answers anyone. A second project shows it.
  const openRoot = path.join(scratch, "inbox-open");
  await cp(root, openRoot, { recursive: true });
  const openActions = JSON.parse(await readFile(path.join(openRoot, "actions.json"), "utf8"));
  delete openActions.actions["events.feed"].http.signature;
  await writeFile(path.join(openRoot, "actions.json"), JSON.stringify(openActions, null, 2));
  const openProject = { id: "showcase_inbox_open", name: "Inbox without a read signature", slug: "inbox-open", manifest: { version: 1, visibility: "private" } };
  await owner.call("project_create", { projectId: openProject.id, name: openProject.name, slug: openProject.slug, manifest: openProject.manifest });
  await owner.call("secret_set", { projectId: openProject.id, name: "events_webhook", value: webhookSecret });
  const openBuild = await buildLocal(openRoot);
  const open = await release(openProject, openBuild.files, openBuild.artifact);
  await owner.call("database_execute", { deploymentId: open.id, operation: "sql", input: { sql: "INSERT INTO events(source, external_id, severity, title, body, received_at) VALUES('payroll','run-11','critical','Payroll export failed','Bank file rejected',0)" } });
  const openShell = await fetch(`${url}/`, { headers: { host: open.hostname } });
  assert.match(await openShell.text(), /<title>Sign in<\/title>/);
  const leaked = await fetch(`${url}/api/events`, { headers: { host: open.hostname, "x-forwarded-for": freshAddress() } });
  assert.equal(leaked.status, 200);
  assert.equal((await leaked.json() as any).result[0].title, "Payroll export failed");
  pass("Unsigned read route on a private app", "With the GET route's signature removed, a request with no session and no key reads the private app's rows while its shell still asks for sign-in. Only a signature, which a key replaces, makes the route key-only.");

  // ------------------------------------------------------- Team by invitation
  const invited = await owner.call<{ policy: any; results: Array<{ email: string; status: string; invitationId: string; acceptable: boolean; delivery: string }> }>(
    "app_invitations_create", { deploymentId: inbox.id, emails: ["ana@team.example", "ben@team.example"], idempotencyKey: "inbox-team-first-batch" });
  assert.deepEqual(invited.results.map((result) => [result.email, result.status, result.acceptable]), [["ana@team.example", "invited", true], ["ben@team.example", "invited", true]]);
  const mailbox = async () => (await readFile(path.join(dataDir, "mailbox.jsonl"), "utf8").catch(() => "")).trim().split("\n").filter(Boolean).map((line) => JSON.parse(line)).filter((message) => message.purpose === "invitation");
  let mails = await mailbox();
  for (const deadline = Date.now() + 10_000; mails.length < 2 && Date.now() < deadline; mails = await mailbox()) await Bun.sleep(100);
  assert.equal(mails.length, 2);
  const linkFor = (email: string) => mails.find((message) => message.to === email);
  // The development mailbox keeps to, purpose, scope, link and expiry; the subject
  // that names the inviting owner is not written, so this run cannot check it.
  assert.deepEqual(Object.keys(linkFor("ana@team.example")).sort(), ["expiresAt", "link", "purpose", "scope", "to"]);
  assert.equal(linkFor("ana@team.example").scope, `deployment:${inbox.id}`);
  const appPost = (route: string, body: object, cookie?: string) => fetch(`${url}${route}`, {
    method: "POST", headers: host({ "content-type": "application/json", ...(cookie ? { cookie } : {}) }), body: JSON.stringify(body),
  });
  const accept = async (email: string) => {
    const token = new URL(linkFor(email).link).hash.slice("#invitation=".length);
    const preview = await (await appPost("/_littlebox/auth/invitation", { token })).json() as any;
    assert.deepEqual(preview.result, { email, app: "Inbox", status: "new-account" });
    const response = await appPost("/_littlebox/auth/accept", { token, password });
    assert.equal(response.status, 200, await response.clone().text());
    return { token, cookie: response.headers.get("set-cookie")!.split(";")[0] };
  };
  const ana = await accept("ana@team.example");
  const ben = await accept("ben@team.example");
  const reused = await appPost("/_littlebox/auth/accept", { token: ana.token, password });
  assert.equal(reused.status, 410);
  refused("An accepted invitation link used again", (await reused.json() as any).error, 410);
  const users = await owner.call<{ users: Array<{ userId: string; email: string; state: string }> }>("app_users_list", { deploymentId: inbox.id });
  assert.deepEqual(users.users.map((user) => [user.email, user.state]), [["ana@team.example", "active"], ["ben@team.example", "active"]]);
  const anaClient = runtime(ana.cookie);
  const benClient = runtime(ben.cookie);
  const anaSession = await anaClient.call<{ userId: string; name: string; role: string }>("auth_session");
  assert.equal(anaSession.name, "ana@team.example");
  for (const [client, email] of [[anaClient, "ana@team.example"], [benClient, "ben@team.example"]] as const) {
    assert.equal((await action(client, "members.hello", { email })).changes, 1);
  }
  const invite = await rejects(anaClient.call("app_invitations_create", { deploymentId: inbox.id, emails: ["x@team.example"], idempotencyKey: "runtime-tries" }), /.+/);
  refused("A member calls app_invitations_create on the runtime endpoint", invite);
  const invitationsList = await owner.call<{ invitations: Array<{ email: string; status: string }> }>("app_invitations_list", { deploymentId: inbox.id });
  assert(invitationsList.invitations.every((invitation) => invitation.status === "accepted"));
  pass("Team by invitation", "The owner invites two addresses, both links arrive in the development mailbox scoped to the Deployment, both accept and land active, a used link answers 410, and a stranger cannot sign up.");

  // ------------------------------------------ Nullable filters and a LIMIT
  const all = await action<any[]>(anaClient, "events.list");
  assert.equal(all.length, 5);
  assert.deepEqual(await action(anaClient, "events.list", { severity: null, state: null }), all);
  const critical = await action<any[]>(anaClient, "events.list", { severity: "critical" });
  assert.deepEqual(critical.map((row) => row.external_id).sort(), ["chk-web", "inv-4471"]);
  const emptyFilter = await rejects(action(anaClient, "events.list", { severity: "" }), "severity must be a non-empty string.");
  refused("events.list with severity set to an empty string", emptyFilter);

  const ack = await action(anaClient, "events.acknowledge", { id: id["inv-4471"] });
  assert.equal(ack.changes, 1);
  assert.deepEqual(await action(anaClient, "events.acknowledged", { id: id["inv-4471"], severity: "critical" }), { recorded: true });
  assert.equal((await action(benClient, "events.acknowledge", { id: id["inv-4471"] })).changes, 0);
  assert.equal((await action(benClient, "events.acknowledge", { id: id["rel-812"] })).changes, 1);
  assert.deepEqual(await action(benClient, "events.acknowledged", { id: id["rel-812"], severity: "info" }), { recorded: true });
  const openOnly = await action<any[]>(anaClient, "events.list", { state: "open" });
  assert.deepEqual(openOnly.map((row) => row.external_id), ["nightly-1106", "chk-web", "chk-api-eu"]);
  const done = await action<any[]>(benClient, "events.list", { state: "acknowledged", severity: "critical" });
  assert.deepEqual(done.map((row) => [row.external_id, row.acknowledged_by_email]), [["inv-4471", "ana@team.example"]]);
  assert.deepEqual(await action(anaClient, "events.counts"), [{ total: 5, acknowledged: 2 }]);
  pass("In-app list", "events.list with {} and with explicit nulls returns the same 5 rows, severity and state filters narrow them, a second acknowledgement changes nothing, and counts follow.");

  // A signed-in member reaches the anonymous webhook action without its signature.
  const memberReceive = await action(benClient, "events.receive", { source: "ben", id: "typed-by-hand", severity: "critical", title: "Typed by a member, no signature" });
  assert.equal(memberReceive.changes, 1);
  // A Signal action does not know whether its mutation ran.
  assert.deepEqual(await action(benClient, "events.acknowledged", { id: 999, severity: "critical" }), { recorded: true });
  // members.hello trusts the email the page sends.
  assert.equal((await action(benClient, "members.hello", { email: "ana@team.example" })).changes, 1);
  await action(benClient, "members.hello", { email: "ben@team.example" });
  pass("What the actions let a member do", "A signed-in member stores an event through actions_invoke on the signed webhook action with no signature, records event.acknowledged for event 999 that no one acknowledged, and can claim another member's email in members.hello.");

  // ------------------------------------------------------------- Suspension
  const benId = users.users.find((user) => user.email === "ben@team.example")!.userId;
  const suspended = await owner.call<{ state: string }>("app_user_suspend", { deploymentId: inbox.id, userId: benId });
  assert.equal(suspended.state, "suspended");
  const benAfter = await rejects(action(benClient, "events.list"), /.+/);
  refused("A suspended member's existing session calls events.list", benAfter);
  const benLogin = await appPost("/_littlebox/auth/login", { email: "ben@team.example", password });
  assert.equal(benLogin.status, 403);
  refused("A suspended member signs in with the right password", (await benLogin.json() as any).error, 403);
  const benShell = await fetch(`${url}/`, { headers: host({ cookie: ben.cookie }) });
  assert.match(await benShell.text(), /<title>Sign in<\/title>/);
  const benAccept = await appPost("/_littlebox/auth/accept", { token: ben.token, password });
  assert.equal(benAccept.status, 410);
  const reinvite = await owner.call<{ results: Array<{ status: string }> }>("app_invitations_create", { deploymentId: inbox.id, emails: ["ben@team.example"], idempotencyKey: "inbox-reinvite-ben" });
  assert.equal(reinvite.results[0].status, "suspended");
  assert.equal((await action<any[]>(anaClient, "events.list")).length, 6);
  const anaLogin = await appPost("/_littlebox/auth/login", { email: "ana@team.example", password });
  assert.equal(anaLogin.status, 200);
  pass("Suspension", "After app_user_suspend, Ben's open session is refused, the right password answers 403, the shell asks for sign-in again, and a new invitation to that address answers suspended. Ana's session and sign-in keep working.");

  // ------------------------------------------------- LIMIT: Gate and Kernel
  const unbounded = JSON.parse(compiled.files["actions.json"]);
  unbounded.actions["events.list"].statement = unbounded.actions["events.list"].statement.replace(/ LIMIT 50$/, "");
  const unboundedSource = JSON.stringify(unbounded, null, 2);
  const gate = createCapsuleValidator(() => initSqlJs());
  const capsule = (actions: string) => ({ ...starterProject.files, ...compiled.files, "actions.json": actions }) as CapsuleFiles;
  assert.equal((await gate.validate(capsule(compiled.files["actions.json"]))).ok, true);
  const gateResult = await gate.validate(capsule(unboundedSource));
  assert.equal(gateResult.ok, false);
  const gateError = gateResult.errors[0];
  const gateText = `${gateError.message} ${gateError.detail ?? ""}`;
  assert.match(gateText, /events\.list reads rows and has no LIMIT/);
  refused("Validation Gate: events.list without LIMIT", gateText);
  const kernelText = await rejects(release(project, { ...compiled.files, "actions.json": unboundedSource }, compiled.artifact), /events\.list reads rows and has no LIMIT/);
  refused("Kernel release_plan: events.list without LIMIT", kernelText);
  const sentence = "Action events.list reads rows and has no LIMIT. Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.";
  assert(gateText.includes(sentence) && kernelText.includes(sentence), "The Gate and the Kernel give the same sentence.");
  pass("LIMIT in both doors", `The Validation Gate and release_plan refuse events.list without a LIMIT with the same sentence: "${sentence}"`);

  // -------------------------------------------------------- Usage and Signals
  const usage = await owner.call<{ totals: Array<{ kind: string; name: string; count: number }> }>("usage_read", { deploymentId: inbox.id, from: today, to: today });
  const total = (kind: string, name = "") => usage.totals.find((row) => row.kind === kind && row.name === name)?.count ?? 0;
  assert.equal(total("route", "POST /hooks/events"), delivered, "Only accepted webhook calls count.");
  assert.equal(total("route", "GET /api/events"), routeReads, "Only keyed reads that ran count.");
  assert.equal(total("action", "events.acknowledged"), 3);
  assert.equal(total("action", "events.receive"), 1, "The member's actions_invoke call counts as an action.");
  assert.equal(total("signup"), 2);
  const received = await owner.call<{ signals: Array<{ name: string; at: number; properties: Record<string, unknown> }> }>("signals_read", { deploymentId: inbox.id, name: "event.received" });
  const acknowledged = await owner.call<{ signals: Array<{ name: string; properties: Record<string, unknown> }> }>("signals_read", { deploymentId: inbox.id, name: "event.acknowledged" });
  assert.equal(received.signals.length, delivered + 1, "Every successful receive records a Signal, the stored-nothing retry included.");
  assert.deepEqual(received.signals.at(-1)!.properties, { source: "billing", severity: "critical" });
  assert.deepEqual(acknowledged.signals.map((signal) => signal.properties.id).sort((a: any, b: any) => a - b), [id["inv-4471"], id["rel-812"], 999]);
  const runtimeUsage = await rejects(anaClient.call("usage_read", { deploymentId: inbox.id, from: today, to: today }), /.+/);
  refused("An app user calls usage_read on the runtime endpoint", runtimeUsage);
  pass("Usage and Signals", `usage_read counts ${delivered} POST /hooks/events and ${routeReads} GET /api/events, none of the refused requests, 2 signups; signals_read returns ${received.signals.length} event.received (the retry that stored nothing included) and 3 event.acknowledged (event 999 included).`);

  // ----------------------------------------------------------------- Browser
  if (process.env.LITTLEBOX_SHOWCASE_BROWSER !== "0") {
    const { chromium } = await import("playwright");
    const shots = process.env.LITTLEBOX_SHOWCASE_SCREENSHOTS;
    if (shots) await mkdir(shots, { recursive: true });
    const [before] = await action<Array<{ total: number; acknowledged: number }>>(anaClient, "events.counts");
    const signalsBefore = (await owner.call<{ signals: unknown[] }>("signals_read", { deploymentId: inbox.id, name: "event.acknowledged" })).signals.length;
    const browser = await chromium.launch({ channel: "chrome" });
    try {
      for (const colorScheme of ["light", "dark"] as const) {
        const context = await browser.newContext({ colorScheme, viewport: { width: 1100, height: 900 } });
        const page = await context.newPage();
        const violations: string[] = [];
        page.on("console", (message) => { if (/Content Security Policy|Refused to/i.test(message.text())) violations.push(message.text()); });
        page.on("pageerror", (error) => violations.push(`page error: ${error.message}`));
        await page.addInitScript(() => document.addEventListener("securitypolicyviolation", (event) =>
          console.error(`Refused to load ${event.blockedURI} by ${event.violatedDirective}`)));
        const response = await page.goto(inbox.url, { waitUntil: "load" });
        assert(response?.headers()["content-security-policy"]?.includes("frame-ancestors 'none'"), "The shell carries the Kernel's policy.");
        await page.getByLabel("Email").fill("ana@team.example");
        await page.getByLabel("Password").fill(password);
        await page.getByRole("button", { name: "Sign in" }).click();
        await page.waitForSelector(".event", { timeout: 15_000 });
        await page.locator(".segmented label", { hasText: "All" }).click();
        await page.waitForFunction((count) => document.querySelectorAll(".event").length === count, Math.min(before.total, 50), { timeout: 10_000 });
        assert.equal(await page.locator("#who").textContent(), "Signed in as ana@team.example");
        assert.equal(await page.locator("#open-count").textContent(), String(before.total - before.acknowledged - (colorScheme === "light" ? 0 : 1)));
        if (colorScheme === "light") {
          // Acknowledge one event from the page, which also records its Signal.
          await page.locator(".event", { hasText: "api.eu answers" }).getByRole("button", { name: "Acknowledge" }).click();
          await page.locator(".event", { hasText: "api.eu answers" }).locator(".acked").waitFor({ timeout: 10_000 });
        }
        assert.deepEqual(violations, [], `The ${colorScheme} page broke under the Content Security Policy.`);
        if (shots) await page.screenshot({ path: path.join(shots, `inbox-${colorScheme}.png`), fullPage: true });
        await context.close();
      }
    } finally { await browser.close(); }
    const afterBrowser = await owner.call<{ signals: unknown[] }>("signals_read", { deploymentId: inbox.id, name: "event.acknowledged" });
    assert.equal(afterBrowser.signals.length, signalsBefore + 1, "The page's Acknowledge button recorded one more Signal.");
    pass("Browser", `Chrome signs in through the Kernel's page, renders ${before.total} events in light and dark, acknowledges one from the page and reports no Content Security Policy violation${shots ? `; screenshots in ${shots}` : ""}.`);
  }

  // ------------------------------------------------- Action Contract, conform
  const contractDir = path.join(scratch, "contract");
  await cp(root, contractDir, { recursive: true });
  const cli = (args: string[]) => Bun.spawn(["bun", "run", path.join(repo, "scripts/littlebox.ts"), ...args], { cwd: repo, stdout: "pipe", stderr: "pipe" });
  const contract = cli(["contract", "--dir", contractDir]);
  assert.equal(await contract.exited, 0, await new Response(contract.stderr).text());
  const written = JSON.parse(await readFile(path.join(contractDir, "action-contract.json"), "utf8"));
  assert.deepEqual(written.actions.map((item: { name: string }) => item.name).sort(),
    ["events.acknowledge", "events.acknowledged", "events.counts", "events.feed", "events.list", "events.receive", "members.hello"]);
  const inputsFile = path.join(scratch, "conform-inputs.json");
  await writeFile(inputsFile, JSON.stringify({
    "events.receive": { severity: "info" },
    "events.list": { severity: "info", state: "open" },
    "events.acknowledged": { severity: "info" },
  }));
  const anaToken = (await appPost("/_littlebox/auth/login", { email: "ana@team.example", password })).headers.get("set-cookie")!.split(";")[0].split("=").slice(1).join("=");
  const conformKey = await owner.call<{ key: string }>("api_keys_create", { deploymentId: inbox.id, name: "Conformance", actions: ["events.feed", "events.receive"] });
  const runConform = async (secrets: string[]) => {
    const child = cli(["conform", "--url", inbox.url, "--contract", path.join(contractDir, "action-contract.json"),
      "--token", anaToken, ...secrets.flatMap((secret) => ["--secret", secret]), "--key", conformKey.key, "--inputs", inputsFile]);
    const [code, out, err] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
    const lines = out.trim().split("\n");
    return { code, out: out + err, totals: lines.at(-1) ?? "", failures: lines.filter((line) => line.startsWith("FAIL")) };
  };
  // The program that reads /api/events holds a key, so conform reads it with the key.
  const keyed = await runConform([`events_webhook=${webhookSecret}`]);
  assert.equal(keyed.code, 0, keyed.out);
  assert.match(keyed.totals, /^\d+ passed, 0 failed, 0 skipped/);
  // Signing the GET route instead: its MAC covers "<t>." and no query string, so two
  // signed GETs inside one second carry the same signature and the second is a replay.
  // A new second, so this run's "not json" probe cannot repeat the previous run's signature.
  await Bun.sleep(1_100);
  const signed = await runConform([`events_webhook=${webhookSecret}`, `events_feed=${feedSecret}`]);
  // Whether a probe lands in the same second as the one before it is timing, so the
  // count may vary; every failure is a 409 replay where the contract expects a 400.
  assert(signed.failures.every((line) => line.includes('Answered 409 "replayed-signature" where the contract says 400')), signed.out);
  for (const line of signed.failures) refused("conform signs GET /api/events instead of using the key", line.slice(5), 409);
  // conform signed GETs moments ago; an earlier t gives a signature no one has sent.
  const feedAt = Math.floor(Date.now() / 1000) - 7;
  const feedSignature = sign("", feedSecret, feedAt);
  const signedFeed = (query: string) => fetch(`${url}/api/events${query}`, { headers: host({ "x-inbox-signature": feedSignature }) });
  assert.equal((await signedFeed("?after=1")).status, 200);
  const otherQuery = await signedFeed("?after=4");
  assert.equal(otherQuery.status, 409, "The signature does not depend on the query, so a second query in the same second is a replay.");
  pass("Action Contract", `littlebox contract writes 7 actions. littlebox conform against the private Deployment, reading the GET route with its key, reports "${keyed.totals}". With the feed secret instead it reports "${signed.totals}": the signed GET's MAC does not cover the query string.`);

  console.log(JSON.stringify({ app: "inbox", checks: passes.length, refusals }, null, 2));
} finally {
  kernel?.kill();
  if (kernel) await kernel.exited;
  await rm(scratch, { recursive: true, force: true });
}
