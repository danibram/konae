// Replays the starter showcase on two Kernels it starts itself. It publishes
// the Capsule from the CLI and from the managed path, then proves each rule
// the README claims: verified open signup, the visitor's public reads, rows
// bound to each member, a team admin promoted by a guarded mutation, an API
// key inside and outside its scope, a signed webhook, the file lifecycle and
// quota, pagination and search under the LIMIT rules, Signals, the Action
// Contract with conform, and a package export imported on the second Kernel.
// It removes both Kernels and their data on exit.
//
//   LITTLEBOX_SHOWCASE_PORT=23450 bun run examples/showcase/starter/replay.ts
//   LITTLEBOX_SHOWCASE_SCREENSHOTS=/tmp/shots   adds light and dark screenshots
//   LITTLEBOX_SHOWCASE_BROWSER=0                skips Chrome
import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { deflateSync } from "node:zlib";
import initSqlJs from "sql.js";
import { chromium } from "playwright";
import { McpClient, type Transfer } from "../../../src/mcp/client";
import { buildLocal } from "../../../scripts/littlebox";
import { createCapsuleValidator } from "../../../src/agent/gate";
import { signRoute } from "../../../src/lib/route-signature";

const repo = path.resolve(import.meta.dir, "../../..");
const capsuleFiles = ["index.html", "styles.css", "app.js", "favicon.svg", "schema.sql", "schema.ts", "actions.json"];
const port = Number(process.env.LITTLEBOX_SHOWCASE_PORT ?? 23450);
assert(Number.isInteger(port) && port >= 1024 && port + 1 <= 65535, "Set LITTLEBOX_SHOWCASE_PORT to a free local port; the replay also uses the next one.");
const scratch = await mkdtemp(path.join(tmpdir(), "littlebox-showcase-starter-"));
const shots = process.env.LITTLEBOX_SHOWCASE_SCREENSHOTS;
const binary = path.join(repo, "kernel-rs/target/debug/littlebox-kernel");
const ownerEmail = "owner@starter.test";
const passwords = { owner: `owner-${crypto.randomUUID()}`, ana: `ana-${crypto.randomUUID()}`, ben: `ben-${crypto.randomUUID()}`, olga: `olga-${crypto.randomUUID()}` };
const people = { ana: "ana@starter.test", ben: "ben@starter.test", olga: ownerEmail };
const webhookSecret = `status-${crypto.randomUUID()}`;
const MiB = 1024 * 1024;

const observations: { scenario: string; result: string }[] = [];
const refusals: { primitive: string; request: string; answer: string }[] = [];
const kernels: ReturnType<typeof Bun.spawn>[] = [];
let address = 0;
const freshAddress = () => `198.18.${Math.floor(++address / 250) % 250}.${(address % 250) + 1}`;
const json = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));

function check(scenario: string, result: string) {
  observations.push({ scenario, result });
  console.log(`PASS ${scenario}. ${result}`);
}
function refused(primitive: string, request: string, answer: string) {
  refusals.push({ primitive, request, answer });
}
/** The message a promise rejects with; fails when it resolves. */
async function message(promise: Promise<unknown>) {
  try { await promise; } catch (error) { return error instanceof Error ? error.message : String(error); }
  assert.fail("Expected a refusal.");
}

// ------------------------------------------------------------------ Kernels

type Kernel = { url: string; data: string; owner: McpClient; ownerCookie: string };

async function startKernel(kernelPort: number, name: string): Promise<Kernel> {
  const url = `http://127.0.0.1:${kernelPort}`;
  const data = path.join(scratch, name);
  const setupKey = crypto.randomUUID() + crypto.randomUUID();
  const child = Bun.spawn([binary], {
    cwd: repo, stdout: "ignore", stderr: "inherit",
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_") && !key.startsWith("VITE_"))),
      LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_KERNEL_PORT: String(kernelPort), LITTLEBOX_PUBLIC_ORIGIN: url,
      LITTLEBOX_HOST_SUFFIX: "localhost", LITTLEBOX_DATA_DIR: data, LITTLEBOX_ADMIN_KEY: setupKey,
      LITTLEBOX_BUN_PATH: process.execPath, LITTLEBOX_SOURCE_ROOT: repo, LITTLEBOX_TRUSTED_PROXIES: "127.0.0.1",
      LITTLEBOX_ACCOUNT_VERIFICATION: "required", LITTLEBOX_MAIL_TRANSPORT: "development",
    },
  });
  kernels.push(child);
  let ready = false;
  for (let attempt = 0; attempt < 150 && !ready; attempt++) {
    assert.equal(child.exitCode, null, `Kernel ${name} stopped during startup. Is port ${kernelPort} free?`);
    try { ready = (await fetch(`${url}/_littlebox/health`)).ok; } catch {}
    if (!ready) await Bun.sleep(100);
  }
  assert(ready, `Kernel ${name} did not start on port ${kernelPort}.`);
  // Required verification covers the creator too: the setup key answers 202 and a link.
  const signup = await post(url, "/_littlebox/owner/signup", { email: ownerEmail, setupKey });
  assert.equal(signup.status, 202, await signup.clone().text());
  const verify = await post(url, "/_littlebox/owner/verify", { token: (await mail(data, ownerEmail, "verification")).token, password: passwords.owner });
  assert.equal(verify.status, 200, await verify.clone().text());
  const ownerCookie = cookieOf(verify)!;
  return { url, data, ownerCookie, owner: client(`${url}/mcp`, { cookie: ownerCookie }) };
}

function post(base: string, route: string, body: unknown, headers: Record<string, string> = {}) {
  return fetch(`${base}${route}`, {
    method: "POST", redirect: "manual",
    headers: { "content-type": "application/json", "x-forwarded-for": freshAddress(), ...headers },
    body: JSON.stringify(body),
  });
}
const cookieOf = (response: Response) => response.headers.get("set-cookie")?.split(";")[0];
const tokenOfCookie = (cookie: string) => cookie.split("=").slice(1).join("=");

/** An MCP client whose every request carries these headers and a fresh client address. */
function client(endpoint: string, headers: Record<string, string>) {
  return new McpClient(endpoint, { fetch: (input, init) => {
    const merged = new Headers(init?.headers);
    for (const [name, value] of Object.entries(headers)) merged.set(name, value);
    merged.set("x-forwarded-for", freshAddress());
    return fetch(input, { ...init, headers: merged });
  } });
}

async function mailbox(data: string) {
  const text = await readFile(path.join(data, "mailbox.jsonl"), "utf8").catch(() => "");
  return text.split("\n").filter(Boolean).map((line) => JSON.parse(line) as { to: string; purpose: string; link: string });
}
const mailCount = async (data: string, to: string, purpose: string) => (await mailbox(data)).filter((item) => item.to === to && item.purpose === purpose).length;

/** Waits for a development email of one purpose to one address, beyond the first `after`, and returns its token. */
async function mail(data: string, to: string, purpose: string, after = 0) {
  for (const deadline = Date.now() + 15_000; Date.now() < deadline; await Bun.sleep(100)) {
    const matches = (await mailbox(data)).filter((item) => item.to === to && item.purpose === purpose);
    if (matches.length > after) {
      const link = matches.at(-1)!.link;
      return { token: new URLSearchParams(new URL(link).hash.slice(1)).get(purpose)!, link, count: matches.length };
    }
  }
  assert.fail(`No ${purpose} email reached ${to}.`);
}

// ---------------------------------------------------------------- one app

type Receipt = { id: string; url: string };

/** Speaks to one Deployment's host as a browser, a program or a sender would. */
function app(kernel: Kernel, receipt: Receipt) {
  const host = new URL(receipt.url).hostname;
  const origin = new URL(receipt.url).origin;
  const http = async (method: string, route: string, init: { body?: string; cookie?: string; bearer?: string; headers?: Record<string, string> } = {}) => {
    const headers = new Headers(init.headers);
    headers.set("host", host);
    headers.set("x-forwarded-for", freshAddress());
    if (init.body !== undefined) headers.set("content-type", "application/json");
    if (init.cookie) headers.set("cookie", init.cookie);
    if (init.bearer) headers.set("authorization", `Bearer ${init.bearer}`);
    return fetch(kernel.url + route, { method, headers, body: init.body, redirect: "manual" });
  };
  /** A route's answer as { status, body }. */
  const route = async (method: "GET" | "POST", routePath: string, init: Parameters<typeof http>[2] = {}) => {
    const response = await http(method, routePath, init);
    return { status: response.status, body: await response.json() as any };
  };
  const runtime = (cookie?: string) => client(`${kernel.url}/_littlebox/mcp`, cookie ? { host, cookie } : { host });
  const auth = (route: string, body: unknown) => post(kernel.url, route, body, { host });
  /** Open signup under required verification: the email, then the link with the password. */
  const signUp = async (email: string, password: string) => {
    const before = await mailCount(kernel.data, email, "verification");
    const started = await auth("/_littlebox/auth/signup", { email });
    assert.equal(started.status, 202, await started.clone().text());
    assert.equal(cookieOf(started), undefined, "A signup must not open a session before verification.");
    const verified = await auth("/_littlebox/auth/verify", { token: (await mail(kernel.data, email, "verification", before)).token, password });
    assert.equal(verified.status, 200, await verified.clone().text());
    return cookieOf(verified)!;
  };
  const login = async (email: string, password: string) => {
    const response = await auth("/_littlebox/auth/login", { email, password });
    return { status: response.status, cookie: response.ok ? cookieOf(response) : undefined, text: response.ok ? "" : await response.text() };
  };
  const sign = (body: string, secret = webhookSecret, t = Math.floor(Date.now() / 1000), nonce?: string) =>
    signRoute(secret, { method: "POST", path: "/hooks/status", body }, t, nonce);
  const hook = (body: string, signature?: string) =>
    route("POST", "/hooks/status", { body, headers: signature ? { "x-starter-signature": signature } : {} });
  const fileGet = (id: string, init: { cookie?: string; bearer?: string } = {}) => http("GET", `/_littlebox/files/${id}`, init);
  return { receipt, host, origin, http, route, runtime, auth, signUp, login, sign, hook, fileGet };
}

const action = <T = any>(caller: McpClient, name: string, input: object = {}) => caller.call<T>("actions_invoke", { name, input });

async function upload(caller: McpClient, bytes: Uint8Array, name: string, type: string, visibility?: "public" | "private") {
  const transfer = await caller.upload(bytes);
  try { return await caller.call<{ id: string; name: string; size: number; visibility: string }>("files_upload", { transferId: transfer.transferId, name, type, ...(visibility ? { visibility } : {}) }); }
  finally { await caller.cancel(transfer.transferId).catch(() => {}); }
}

function cli(args: string[], env: Record<string, string> = {}) {
  const child = Bun.spawn(["bun", "run", path.join(repo, "scripts/littlebox.ts"), ...args], {
    cwd: repo, stdout: "pipe", stderr: "pipe",
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("LITTLEBOX_"))), ...env },
  });
  return Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
}

// A real PNG so the avatar renders in Chrome.
function crc32(bytes: Uint8Array) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    let c = (crc ^ byte) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = (crc >>> 8) ^ c;
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function png(size: number, paint: (x: number, y: number) => [number, number, number]) {
  const raw = Buffer.alloc((size * 3 + 1) * size);
  for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) raw.set(paint(x, y), y * (size * 3 + 1) + 1 + x * 3);
  const chunk = (type: string, data: Buffer) => {
    const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
    const out = Buffer.alloc(body.length + 8);
    out.writeUInt32BE(data.length, 0); body.copy(out, 4); out.writeUInt32BE(crc32(body), body.length + 4);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0); header.writeUInt32BE(size, 4); header.set([8, 2, 0, 0, 0], 8);
  return new Uint8Array(Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", header), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]));
}
const anaAvatar = png(64, (x, y) => ((x >> 3) + (y >> 3)) % 2 ? [200, 243, 74] : [32, 35, 28]);

try {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
  assert.equal(await build.exited, 0, "The Kernel must compile.");
  const sources = Object.fromEntries(await Promise.all(capsuleFiles.map(async (name) => [name, await readFile(path.join(import.meta.dir, name), "utf8")] as const)));
  const program = JSON.parse(sources["actions.json"]);

  // ------------------------------------------------- the Gate and the LIMIT rule
  const gate = createCapsuleValidator(() => initSqlJs());
  const passed = await gate.validate({ "schema.sql": sources["schema.sql"], "actions.json": sources["actions.json"] } as never);
  assert.equal(passed.ok, true, JSON.stringify(passed.errors));
  const unbounded = structuredClone(program);
  unbounded.actions["notes.list"].statement = program.actions["notes.list"].statement.replace(/ LIMIT 20$/, "");
  const gateUnbounded = await gate.validate({ "schema.sql": sources["schema.sql"], "actions.json": JSON.stringify(unbounded) } as never);
  assert.equal(gateUnbounded.ok, false);
  const gateLimitRefusal = gateUnbounded.errors[0].detail ?? gateUnbounded.errors[0].message;
  assert.match(gateLimitRefusal, /^Action notes\.list reads rows and has no LIMIT/);

  const A = await startKernel(port, "origin");
  const B = await startKernel(port + 1, "destination");

  // The Kernel refuses the same statement when a release carries it.
  await A.owner.call("project_create", { projectId: "starter_probe", name: "Starter probe", slug: "starter-probe", manifest: { version: 1, visibility: "public" } });
  const probe = { project: { id: "starter_probe", name: "Starter probe", slug: "starter-probe", manifest: { version: 1, visibility: "public" }, files: { ...sources, "actions.json": JSON.stringify(unbounded) } }, visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>probe</main>" } } };
  const probeUpload = await A.owner.upload(json(probe), "starter_probe");
  const kernelLimitRefusal = await message(A.owner.call("release_plan", { transferId: probeUpload.transferId }));
  await A.owner.cancel(probeUpload.transferId).catch(() => {});
  assert.equal(kernelLimitRefusal, gateLimitRefusal);
  refused("LIMIT rule", "notes.list without its LIMIT, in the Gate and on release_plan", kernelLimitRefusal);
  check("Gate", `The Gate passes the Capsule's 27 actions. notes.list without its LIMIT is refused by the Gate and by the Kernel's release_plan with the same words: "${kernelLimitRefusal}"`);

  // ------------------------------------------------------- the CLI publishes
  const cliDir = path.join(scratch, "cli");
  await mkdir(cliDir);
  const ownerEnv = { LITTLEBOX_EMAIL: ownerEmail, LITTLEBOX_PASSWORD: passwords.owner };
  const [loginCode, loginOut, loginErr] = await cli(["login", "--dir", cliDir, "--url", A.url], ownerEnv);
  assert.equal(loginCode, 0, loginOut + loginErr);
  const [initCode, initOut, initErr] = await cli(["init", "--dir", cliDir, "--name", "Starter", "--url", A.url]);
  assert.equal(initCode, 0, initOut + initErr);
  const config = JSON.parse(await readFile(path.join(cliDir, "littlebox.json"), "utf8"));
  const initPolicy = config.project.manifest.signupPolicy.mode;
  const initVisibility = config.project.manifest.visibility;
  // Replace the template with the Capsule and ask for a public app with open signup.
  for (const name of await readdir(cliDir)) if (!name.startsWith(".") && name !== "littlebox.json") await rm(path.join(cliDir, name), { recursive: true });
  for (const name of capsuleFiles) await cp(path.join(import.meta.dir, name), path.join(cliDir, name));
  config.project.manifest = { ...config.project.manifest, visibility: "public", signupPolicy: { mode: "open" } };
  await writeFile(path.join(cliDir, "littlebox.json"), JSON.stringify(config, null, 2));
  const projectId: string = config.project.id;
  const [unsetCode, , unsetErr] = await cli(["publish", "--dir", cliDir]);
  assert.notEqual(unsetCode, 0);
  const unsetSecret = "Action status.report signs with the secret status_webhook, which is not set for this project. Set it with secret_set before publishing.";
  assert(unsetErr.includes(unsetSecret), unsetErr);
  refused("Signed route", "littlebox publish before secret_set", unsetSecret);
  await A.owner.call("secret_set", { projectId, name: "status_webhook", value: webhookSecret });
  const [buildCode, buildOut, buildErr] = await cli(["build", "--dir", cliDir]);
  assert.equal(buildCode, 0, buildOut + buildErr);
  const [publishCode, publishOut, publishErr] = await cli(["publish", "--dir", cliDir]);
  assert.equal(publishCode, 0, publishOut + publishErr);
  const receipt = JSON.parse(await readFile(path.join(cliDir, ".littlebox/deployment.json"), "utf8")) as Receipt;
  assert.equal(publishOut.trim().split("\n").at(-1), receipt.url);
  const S = app(A, receipt);
  const policy = await A.owner.call<{ mode: string }>("signup_policy_read", { deploymentId: receipt.id });
  assert.equal(policy.mode, "open");
  check("CLI publish", `littlebox login, init, build and publish ran against the local Kernel. init wrote a ${initVisibility} app with signup ${initPolicy}; with littlebox.json set to public and open, publish was refused until secret_set, then served ${receipt.url} with signup ${policy.mode}.`);

  // --------------------------------------------- the managed path publishes too
  const managedId = "showcase_starter_managed";
  const manifest = {
    version: 1, visibility: "public",
    runtime: {
      database: { mode: "declared-actions", actions: Object.entries(program.actions).map(([name, item]: [string, any]) => ({ name, kind: item.kind, access: item.access })) },
      auth: { mode: "optional" }, files: { access: "deployment" },
    },
  };
  const managedSource = { id: managedId, name: "Starter", slug: "starter-managed", files: sources, directories: [], databaseModel: { source: sources["schema.ts"], acceptedSource: sources["schema.ts"], revision: "r1" }, manifest, updatedAt: Date.now() };
  const created = await A.owner.call<{ sourceAuthority: string }>("project_create", { projectId: managedId, name: "Starter", slug: "starter-managed", manifest, sourceAuthority: "managed" });
  assert.equal(created.sourceAuthority, "managed");
  const saveUpload = await A.owner.upload(json(managedSource), managedId);
  const saved = await A.owner.call<{ revision: number }>("project_sources_save", { projectId: managedId, transferId: saveUpload.transferId, expectedRevision: 0 });
  await A.owner.cancel(saveUpload.transferId).catch(() => {});
  assert.deepEqual(saved, { revision: 1 });
  await A.owner.call("secret_set", { projectId: managedId, name: "status_webhook", value: webhookSecret });
  const managedRoot = path.join(scratch, "managed");
  await mkdir(managedRoot);
  for (const name of capsuleFiles) await writeFile(path.join(managedRoot, name), sources[name]);
  const { artifact: managedArtifact } = await buildLocal(managedRoot);
  const releaseUpload = await A.owner.upload(json({ project: managedSource, visibility: "public", artifact: managedArtifact, initialSignupPolicy: { mode: "open", allowlist: [] } }), managedId);
  const managedPlan = await A.owner.call<{ planId: string }>("release_plan", { transferId: releaseUpload.transferId });
  const managedReceipt = await A.owner.call<Receipt>("release_apply", { planId: managedPlan.planId });
  await A.owner.cancel(releaseUpload.transferId).catch(() => {});
  const M = app(A, managedReceipt);
  // Every file the page loads, from both doors. The bundler names the folder it built from in a
  // comment, so that line is the only one allowed to differ.
  const builtFrom = /^\/\/ (.*)\/app\.js$/m;
  let cliFolder = "";
  let servedBytes = 0;
  for (const file of ["/", "/app.js", "/styles.css", "/favicon.svg"]) {
    const fromCli = await (await S.http("GET", file)).text();
    const fromManaged = await (await M.http("GET", file)).text();
    cliFolder ||= fromCli.match(builtFrom)?.[1] ?? "";
    assert.equal(fromManaged.replace(builtFrom, ""), fromCli.replace(builtFrom, ""), `${file} must be the same from both doors.`);
    servedBytes += fromCli.length;
  }
  assert(cliFolder.endsWith("/cli"), "The CLI's app.js names the folder it was built from.");
  const upward = cliFolder.match(/^(\.\.\/)+/)![0].length / 3;
  refused("Build Artifact", "littlebox build of app.js", `The published app.js starts with a bundler comment naming the build folder, ${upward} levels up from the repository: "// ${cliFolder.replace(/^(\.\.\/)+/, "../".repeat(upward)).replace(/\/var\/folders\/.*\/(littlebox-showcase-starter-)/, "/var/folders/…/$1")}/app.js"`);
  check("Managed publish", `project_sources_save reached revision ${saved.revision} and release_plan/release_apply served ${managedReceipt.url}. Its page, app.js, styles.css and favicon.svg (${servedBytes} bytes) equal the CLI deployment's, except for the bundler comment naming the folder each was built from.`);

  // ----------------------------------------------- open signup, verification required
  const anaStart = await S.auth("/_littlebox/auth/signup", { email: people.ana });
  assert.equal(anaStart.status, 202);
  assert.equal(cookieOf(anaStart), undefined);
  const pendingLogin = await S.login(people.ana, passwords.ana);
  assert.equal(pendingLogin.status, 401, pendingLogin.text);
  refused("Required verification", "Ana signs in before opening her verification link", `${pendingLogin.status} ${pendingLogin.text}`);
  const pendingUsers = (await A.owner.call<{ users: { email: string; state: string }[] }>("app_users_list", { deploymentId: receipt.id })).users;
  assert.deepEqual(pendingUsers.map((user) => [user.email, user.state]), [[people.ana, "pending"]]);
  const anaVerified = await S.auth("/_littlebox/auth/verify", { token: (await mail(A.data, people.ana, "verification")).token, password: passwords.ana });
  assert.equal(anaVerified.status, 200);
  const cookies = { ana: cookieOf(anaVerified)!, ben: await S.signUp(people.ben, passwords.ben), olga: await S.signUp(people.olga, passwords.olga) };
  const ana = S.runtime(cookies.ana), ben = S.runtime(cookies.ben), olga = S.runtime(cookies.olga);
  const sessions = {
    ana: await ana.call<{ userId: string; role: string }>("auth_session"),
    ben: await ben.call<{ userId: string; role: string }>("auth_session"),
    olga: await olga.call<{ userId: string; name: string; role: string }>("auth_session"),
  };
  // The Project Owner's own email signs up as an ordinary app account.
  assert.equal(sessions.olga.name, ownerEmail);
  refused("Owner inside the app", "auth_session for the app account the Project Owner created with the owner's email", `role "${sessions.olga.role}"; no binding or session field tells the app this account is its owner`);
  check("Verified open signup", `Signup answers 202 with no cookie, the pending account's sign-in answers ${pendingLogin.status}, and the emailed link sets the password and opens a session. Ana, Ben and the owner's app account are active; auth_session gives the owner's account role "${sessions.olga.role}".`);

  // Each person joins the app once; a second join changes nothing.
  for (const [who, caller] of [["ana", ana], ["ben", ben], ["olga", olga]] as const) {
    assert.equal((await action(caller, "me.join", { displayName: who })).changes, 1);
    assert.equal((await action(caller, "me.join", { displayName: `${who} again` })).changes, 0);
  }

  // ------------------------------------------------- the owner's role, then a team admin
  const consoleKey = await A.owner.call<{ key: string; keyId: string }>("api_keys_create", { deploymentId: receipt.id, name: "Owner console", actions: ["owners.appoint"] });
  const appointed = await S.route("POST", "/api/owners", { body: JSON.stringify({ userId: sessions.olga.userId }), bearer: consoleKey.key });
  assert.deepEqual(appointed, { status: 200, body: { result: { changes: 1, lastInsertId: 0 } } });
  const appointAgain = await S.route("POST", "/api/owners", { body: JSON.stringify({ userId: sessions.olga.userId }), bearer: consoleKey.key });
  assert.deepEqual(appointAgain, { status: 403, body: { error: program.actions["owners.appoint"].guard } });
  assert.equal((await action(olga, "me.get"))[0].role, "owner");

  const benPromotes = await message(action(ben, "members.promote", { userId: sessions.ben.userId }));
  assert.equal(benPromotes, program.actions["members.promote"].guard);
  refused("Guard", "Ben (member) promotes himself with members.promote", benPromotes);
  assert.deepEqual(await action(ana, "team.roster"), []);
  const anaPostsEarly = await message(action(ana, "team.post", { body: "Hello team" }));
  assert.equal(anaPostsEarly, program.actions["team.post"].guard);
  assert.equal((await action(olga, "members.promote", { userId: sessions.ana.userId })).changes, 1);
  const roster = await action(ana, "team.roster");
  assert.deepEqual(roster.map((row: any) => [row.display_name, row.role]), [["ana", "admin"], ["olga", "owner"]]);
  assert.equal((await action(ana, "team.post", { body: "Release on Friday. Ben's notes stay his." })).changes, 1);
  assert.equal((await action(ana, "team.posts", { before: null })).length, 1);
  const adminPromotes = await message(action(ana, "members.promote", { userId: sessions.ben.userId }));
  assert.equal(adminPromotes, program.actions["members.promote"].guard);
  refused("Guard", "Ana (team admin) promotes Ben", adminPromotes);
  assert.deepEqual(await action(ana, "members.list", { q: null }), []);
  assert.deepEqual(await action(ana, "admin.stats"), []);
  assert.deepEqual(await action(ben, "team.roster"), []);
  assert.deepEqual(await action(ben, "team.posts", { before: null }), []);
  const benPosts = await message(action(ben, "team.post", { body: "Can I see this?" }));
  assert.equal(benPosts, program.actions["team.post"].guard);
  refused("Guard", "Ben (member) posts in the team area", benPosts);
  const [stats] = await action(olga, "admin.stats");
  assert.equal(stats.admins, 1);
  check("Owner and team admin", `An API key named "Owner console", scoped to owners.appoint, makes the owner's app account the in-app owner over POST /api/owners; a second call answers 403 with the guard. Ben's members.promote is refused with "${benPromotes}"; Ana sees an empty roster and is refused a team post until the owner promotes her with members.promote, a guarded mutation. Then team.roster lists ana (admin) and olga (owner), her post lands, and her own members.promote, members.list and admin.stats get the guard or no rows. Ben still reads no team row.`);

  // ------------------------------------------------------- the visitor's view
  const visitor = S.runtime();
  assert.equal((await visitor.call<{ role: string }>("auth_session")).role, "anonymous");
  const visitorNotes = await message(action(visitor, "notes.list", { before: null, q: null }));
  assert.equal(visitorNotes, "authentication-required");
  refused("Visitor", "actions_invoke notes.list without a session", visitorNotes);
  assert.equal(await message(action(visitor, "team.roster")), "authentication-required");
  const visitorJoin = await message(action(visitor, "me.join", { displayName: "mallory" }));
  assert.equal(visitorJoin, "authentication-required");
  const visitorWrite = await S.route("POST", "/api/updates", { body: JSON.stringify({ title: "Spam" }) });
  assert.deepEqual(visitorWrite, { status: 401, body: { error: "authentication-required" } });
  refused("Private write API", "A visitor's POST /api/updates with no key", `${visitorWrite.status} ${JSON.stringify(visitorWrite.body)}`);
  const visitorOwner = await S.route("POST", "/api/owners", { body: JSON.stringify({ userId: sessions.ben.userId }) });
  assert.equal(visitorOwner.status, 401);
  // A member's session from the app's own origin does not open a key-only route either.
  const benOnKeyRoute = await S.route("POST", "/api/updates", { body: JSON.stringify({ title: "From Ben" }), cookie: cookies.ben, headers: { origin: S.origin } });
  assert.deepEqual(benOnKeyRoute, { status: 401, body: { error: "authentication-required" } });
  const visitorAdmin = await message(visitor.call("app_users_list", { deploymentId: receipt.id }));
  refused("Visitor", "app_users_list on the runtime endpoint", visitorAdmin);

  // ------------------------------------------ the private write API with a key
  const bot = await A.owner.call<{ key: string; keyId: string }>("api_keys_create", { deploymentId: receipt.id, name: "Release bot", actions: ["updates.post"] });
  for (let n = 1; n <= 12; n++) {
    const posted = await S.route("POST", "/api/updates", { body: JSON.stringify({ title: `Release 1.${n}`, body: n % 3 === 0 ? "Includes a fix for search." : null }), bearer: bot.key });
    assert.equal(posted.status, 200, JSON.stringify(posted));
    assert.equal(posted.body.result.changes, 1);
  }
  const outOfScope = await S.route("POST", "/api/owners", { body: JSON.stringify({ userId: sessions.ben.userId }), bearer: bot.key });
  assert.deepEqual(outOfScope, { status: 403, body: { error: "This API key may not call owners.appoint." } });
  refused("API key scope", "The Release bot key, scoped to updates.post, on POST /api/owners", `${outOfScope.status} ${outOfScope.body.error}`);
  const keyOnHook = await S.route("POST", "/hooks/status", { body: JSON.stringify({ component: "api", state: "down" }), bearer: bot.key });
  assert.deepEqual(keyOnHook, { status: 403, body: { error: "This API key may not call status.report." } });
  refused("API key scope", "The Release bot key on the signed webhook, which takes a signature or a key", `${keyOnHook.status} ${keyOnHook.body.error}`);
  const keyExtraField = await S.route("POST", "/api/updates", { body: JSON.stringify({ title: "Release 2.0", author: "someone else" }), bearer: bot.key });
  assert.deepEqual(keyExtraField, { status: 400, body: { error: "Unexpected action input: author" } });
  refused("API key", "POST /api/updates with an undeclared author field", `${keyExtraField.status} ${keyExtraField.body.error}`);
  const keyOnMcp = await S.http("POST", "/_littlebox/mcp", { bearer: bot.key, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }) });
  const keyOnMcpText = await keyOnMcp.text();
  assert.equal(keyOnMcp.status, 403);
  refused("API key", "The Release bot key on the runtime MCP endpoint", `${keyOnMcp.status} ${JSON.parse(keyOnMcpText).error}`);
  const unknownKey = await S.route("POST", "/api/updates", { body: JSON.stringify({ title: "x" }), bearer: "lbk_not_a_real_key" });
  assert.deepEqual(unknownKey, { status: 401, body: { error: "invalid-api-key" } });
  const botRows = (await A.owner.call<{ rows: { author_id: string; source: string }[] }>("database_execute", { deploymentId: receipt.id, operation: "sql", input: { sql: "SELECT DISTINCT author_id, source FROM updates" } })).rows;
  assert.deepEqual(botRows, [{ author_id: bot.keyId, source: "program" }]);
  check("Private write API", `A key named "Release bot", scoped to updates.post, writes 12 updates over POST /api/updates; each row's author_id is the key's ID ${bot.keyId}. The same key is refused on POST /api/owners and on the webhook with 403 "This API key may not call <action>.", an undeclared field answers 400, the runtime endpoint answers 403, an unknown key 401, and a visitor or Ben's session 401 authentication-required.`);

  // ---------------------------------------- pagination and search, public and private
  const page1 = await S.route("GET", "/api/updates");
  assert.equal(page1.status, 200);
  assert.equal(page1.body.result.length, 10);
  // Every column the SELECT names arrives, with the keys in alphabetical order rather than the SELECT's.
  const routeKeys = Object.keys(page1.body.result[0]);
  assert.deepEqual([...routeKeys].sort(), ["author", "body", "id", "published_at", "source", "title"]);
  refused("Public read API", "GET /api/updates, whose SELECT lists id, title, body, source, published_at, author", `Each row's keys arrive as ${routeKeys.join(", ")}`);
  const page2 = await S.route("GET", `/api/updates?before=${page1.body.result.at(-1).id}`);
  assert.deepEqual(page2.body.result.map((row: any) => row.title), ["Release 1.2", "Release 1.1"]);
  const searched = await S.route("GET", "/api/updates?q=search");
  assert.deepEqual(searched.body.result.map((row: any) => row.title), ["Release 1.12", "Release 1.9", "Release 1.6", "Release 1.3"]);
  const repeated = await S.route("GET", "/api/updates?q=a&q=b");
  assert.deepEqual(repeated, { status: 400, body: { error: "Repeated query parameter: q" } });
  refused("Public read API", "GET /api/updates?q=a&q=b", `${repeated.status} ${repeated.body.error}`);
  const mistyped = await S.route("GET", "/api/updates?before=last");
  assert.equal(mistyped.status, 400);
  refused("Public read API", "GET /api/updates?before=last", `${mistyped.status} ${mistyped.body.error}`);
  const headRoute = await S.http("HEAD", "/api/updates");
  assert.equal(headRoute.status, 405);
  // Ana's 25 notes, read 20 at a time; Ben's one note never shows up in hers.
  for (let n = 1; n <= 25; n++) assert.equal((await action(ana, "notes.add", { title: `Note ${n}`, body: n === 7 ? "Ask about the invoice" : null })).changes, 1);
  assert.equal((await action(ben, "notes.add", { title: "Ben's private note", body: "invoice 4471" })).changes, 1);
  const notes1 = await action(ana, "notes.list", { before: null, q: null });
  const notes2 = await action(ana, "notes.list", { before: notes1.at(-1).id, q: null });
  assert.deepEqual([notes1.length, notes2.length, notes2.at(-1).title], [20, 5, "Note 1"]);
  const anaInvoice = await action(ana, "notes.list", { before: null, q: "invoice" });
  assert.deepEqual(anaInvoice.map((row: any) => row.title), ["Note 7"]);
  check("Pagination and search", `GET /api/updates answers 10 of 12 rows with the columns id, title, body, source, published_at and author; ?before=<last id> answers the other 2 and ?q=search the 4 that mention it. A repeated parameter answers 400 "${repeated.body.error}", a word for a number 400 "${mistyped.body.error}", HEAD 405. Ana's 25 notes come back 20 and 5, and her search for "invoice" finds her note and not Ben's.`);

  // ------------------------------------------------------- rows bound to each member
  const benNotes = await action(ben, "notes.list", { before: null, q: null });
  assert.deepEqual(benNotes.map((row: any) => row.title), ["Ben's private note"]);
  const benDeletes = await message(action(ben, "notes.remove", { id: notes1[0].id }));
  assert.equal(benDeletes, program.actions["notes.remove"].guard);
  refused("Guard", "Ben deletes one of Ana's notes by its ID", benDeletes);
  const benAsks = await message(action(ben, "notes.list", { before: null, q: null, userId: sessions.ana.userId }));
  assert.equal(benAsks, "Unexpected action input: userId");
  refused("Actor binding", "Ben's notes.list with a userId input", benAsks);
  assert.equal((await action(ana, "notes.list", { before: null, q: null })).length, 20);
  assert.equal((await action(ana, "notes.remove", { id: notes1[0].id })).changes, 1);
  check("Rows bound to each member", `Ben reads only his own note; deleting one of Ana's answers "${benDeletes}"; naming her user ID answers "${benAsks}". Ana deletes her own.`);

  // --------------------------------------------------------- the signed webhook
  const operational = JSON.stringify({ component: "api", state: "operational", detail: "All systems normal" });
  const firstSignature = await S.sign(operational);
  assert.deepEqual(await S.hook(operational, firstSignature), { status: 200, body: { result: { changes: 1, lastInsertId: 1 } } });
  const replayed = await S.hook(operational, firstSignature);
  assert.deepEqual(replayed, { status: 409, body: { error: "replayed-signature" } });
  refused("Signed webhook", "The same signed delivery sent twice", `${replayed.status} ${replayed.body.error}`);
  const unsigned = await S.hook(operational);
  assert.deepEqual(unsigned, { status: 401, body: { error: "invalid-signature" } });
  refused("Signed webhook", "An unsigned delivery", `${unsigned.status} ${unsigned.body.error}`);
  const stale = await S.hook(operational, await S.sign(operational, webhookSecret, Math.floor(Date.now() / 1000) - 400));
  assert.deepEqual(stale, { status: 401, body: { error: "stale-timestamp" } });
  refused("Signed webhook", "A correct signature whose t is 400 seconds old", `${stale.status} ${stale.body.error}`);
  const forged = await S.hook(operational, await S.sign(operational, "a-guess-at-the-secret-value"));
  assert.deepEqual(forged, { status: 401, body: { error: "invalid-signature" } });
  const degraded = JSON.stringify({ component: "api", state: "degraded", detail: "Slow uploads" });
  const lastSignature = await S.sign(degraded);
  assert.equal((await S.hook(degraded, lastSignature)).body.result.changes, 1);
  // A retry signed again with a new nonce is a new request, and it changes nothing.
  assert.equal((await S.hook(degraded, await S.sign(degraded))).body.result.changes, 0);
  const badState = JSON.stringify({ component: "api", state: "on fire" });
  const checkFailed = await S.hook(badState, await S.sign(badState));
  refused("Signed webhook", "A signed delivery with state \"on fire\", which schema.sql's CHECK refuses", `${checkFailed.status} ${JSON.stringify(checkFailed.body)}`);
  const publicStatus = await action(visitor, "status.list");
  assert.deepEqual(publicStatus.map((row: any) => [row.component, row.state, row.detail]), [["api", "degraded", "Slow uploads"]]);
  check("Signed webhook", `POST /hooks/status signed with status_webhook inserts the api row and a second delivery updates it to degraded, which the visitor reads with status.list. Replayed answers ${replayed.status}, unsigned ${unsigned.status} ${unsigned.body.error}, stale ${stale.status} ${stale.body.error}, another secret ${forged.status}. An identical delivery with a fresh nonce changes 0 rows.`);

  // ------------------------------------------------------------------- files
  const avatar = await upload(ana, anaAvatar, "ana.png", "image/png", "public");
  assert.equal((await action(ana, "me.avatar", { fileId: avatar.id })).changes, 1);
  const contract = new TextEncoder().encode("Draft contract: payment in 30 days.");
  const draft = await upload(ana, contract, "contract-draft.txt", "text/plain");
  assert.equal(draft.visibility, "private");
  assert.equal((await action(ana, "attachments.add", { fileId: draft.id, name: draft.name, size: draft.size })).changes, 1);
  const brochure = await upload(ana, new TextEncoder().encode("Starter brochure, version 1."), "brochure.txt", "text/plain");
  assert.equal((await action(ana, "attachments.add", { fileId: brochure.id, name: brochure.name, size: brochure.size })).changes, 1);
  const photos = await upload(ana, new Uint8Array(2 * MiB).fill(7), "site-photos.bin", "application/octet-stream");
  assert.equal((await action(ana, "attachments.add", { fileId: photos.id, name: photos.name, size: photos.size })).changes, 1);
  const [photosRow, brochureRow, draftRow] = await action(ana, "attachments.list");

  // The avatar is public: a visitor reads it inline.
  const visitorAvatar = await S.fileGet(avatar.id);
  assert.equal(visitorAvatar.status, 200);
  assert.equal(visitorAvatar.headers.get("content-type"), "image/png");
  assert.match(visitorAvatar.headers.get("content-disposition") ?? "", /^inline/);
  // The attachment is private: a visitor and Ben get the same 404 as for a missing file.
  const visitorDraft = await S.fileGet(draft.id);
  const visitorDraftText = await visitorDraft.text();
  assert.equal(visitorDraft.status, 404);
  refused("Private attachment", "A visitor's GET /_littlebox/files/<Ana's draft>", `${visitorDraft.status} ${visitorDraftText}`);
  assert.equal((await S.fileGet(draft.id, { cookie: cookies.ben })).status, 404);
  assert.equal((await S.fileGet(crypto.randomUUID(), { cookie: cookies.ben })).status, 404);
  const anaDraft = await S.fileGet(draft.id, { cookie: cookies.ana });
  assert.equal(anaDraft.status, 200);
  assert.match(anaDraft.headers.get("content-disposition") ?? "", /^attachment/);
  const benChangesDraft = await message(ben.call("files_update", { fileId: draft.id, visibility: "public" }));
  assert.equal(benChangesDraft, "File not found.");
  const benDeletesAvatar = await message(ben.call("files_delete", { fileId: avatar.id }));
  assert.equal(benDeletesAvatar, "Only the person who uploaded this file may change or delete it.");
  refused("File owner", "Ben's files_delete on Ana's public avatar", benDeletesAvatar);
  refused("File owner", "Ben's files_update on Ana's private draft", benChangesDraft);
  const benPublishes = await message(action(ben, "attachments.publish", { id: brochureRow.id }));
  assert.equal(benPublishes, program.actions["attachments.publish"].guard);
  // A key with no file scope reads nothing, not even a public avatar.
  const botAvatar = await S.fileGet(avatar.id, { bearer: bot.key });
  const botAvatarText = await botAvatar.text();
  refused("API key files", "The Release bot key, which names no file visibility, reads Ana's public avatar", `${botAvatar.status} ${botAvatarText}`);

  // Publishing is two steps: the bytes become public, then the row is listed.
  await ana.call("files_update", { fileId: brochure.id, visibility: "public" });
  assert.equal((await action(ana, "attachments.publish", { id: brochureRow.id })).changes, 1);
  const publishTwice = await message(action(ana, "attachments.publish", { id: brochureRow.id }));
  assert.equal(publishTwice, program.actions["attachments.publish"].guard);
  const downloads = await action(visitor, "downloads.list");
  assert.deepEqual(downloads.map((row: any) => [row.name, row.owner_name]), [["brochure.txt", "ana"]]);
  const visitorBrochure = await S.fileGet(brochure.id);
  assert.equal(visitorBrochure.status, 200);
  assert.equal(await visitorBrochure.text(), "Starter brochure, version 1.");
  const visitorList = (await visitor.call<{ id: string }[]>("files_list")).map((file) => file.id).sort();
  assert.deepEqual(visitorList, [avatar.id, brochure.id].sort());
  assert.deepEqual(await action(visitor, "downloads.opened", { fileId: brochure.id }), { recorded: true });

  // Fill the 20 MiB quota with Ben's 2 MiB files until the Kernel refuses one.
  const filler = new Uint8Array(2 * MiB).fill(1);
  const fillers: string[] = [];
  let quotaRefusal = "";
  while (!quotaRefusal) {
    try { fillers.push((await upload(ben, filler, `filler-${fillers.length}.bin`, "application/octet-stream")).id); }
    catch (error) { quotaRefusal = (error as Error).message; }
    assert(fillers.length <= 10, "The quota never refused an upload.");
  }
  assert.equal(quotaRefusal, "This deployment has reached its 20 MiB file quota.");
  refused("File quota", `Ben's upload of a ${fillers.length + 1}th 2 MiB file once the app holds ${fillers.length * 2} MiB of his and 2 MiB of Ana's`, quotaRefusal);
  // Deleting Ana's 2 MiB attachment frees the room, and the same upload goes through.
  assert.deepEqual(await ana.call("files_delete", { fileId: photos.id }), { id: photos.id, deleted: true });
  assert.equal((await action(ana, "attachments.remove", { id: photosRow.id })).changes, 1);
  fillers.push((await upload(ben, filler, `filler-${fillers.length}.bin`, "application/octet-stream")).id);
  const goneForAna = await S.fileGet(photos.id, { cookie: cookies.ana });
  assert.equal(goneForAna.status, 404);
  assert.equal(await message(ana.call("files_read", { fileId: photos.id })), "File not found.");
  for (const id of fillers) await ben.call("files_delete", { fileId: id });
  assert.deepEqual((await action(ana, "attachments.list")).map((row: any) => [row.name, row.public]), [["brochure.txt", 1], ["contract-draft.txt", 0]]);
  void draftRow;
  check("Files", `Ana's avatar is public and a visitor reads it inline; her draft is private, so a visitor and Ben get 404 "File not found." as for a missing file, and Ben's files_update and files_delete are refused. Publishing the brochure takes files_update and attachments.publish, then downloads.list and a visitor read it. ${fillers.length - 1} of Ben's 2 MiB files plus Ana's 2 MiB attachment fill the quota and the next upload answers "${quotaRefusal}"; Ana deletes her attachment, the file path and files_read answer 404 and "File not found.", and Ben's same upload succeeds.`);

  // --------------------------------------- a guard and its refusal, for the page
  const renameTaken = await message(action(ana, "me.rename", { displayName: "Ben" }));
  assert.equal(renameTaken, program.actions["me.rename"].guard);
  assert.equal((await action(ana, "me.rename", { displayName: "Ana" })).changes, 1);

  // ------------------------------------------------------------ Signals and Usage
  const signals: { name: string; properties: Record<string, unknown> }[] = [];
  for (let cursor: string | null = null; ;) {
    const page = await A.owner.call<{ signals: typeof signals; nextCursor: string | null }>("signals_read", { deploymentId: receipt.id, ...(cursor ? { cursor } : {}) });
    signals.push(...page.signals);
    if (!page.nextCursor) break;
    cursor = page.nextCursor;
  }
  const named = (name: string) => signals.filter((signal) => signal.name === name);
  assert.deepEqual(named("member.joined").map((signal) => signal.properties.actor).sort(), [sessions.ana.userId, sessions.ben.userId, sessions.olga.userId].sort());
  assert.equal(named("note.added").length, 26);
  assert.equal(named("note.added").filter((signal) => signal.properties.actor === sessions.ben.userId).length, 1);
  const posted = named("update.posted");
  assert.equal(posted.length, 12);
  assert(posted.every((signal) => signal.properties.actor === bot.keyId));
  assert.deepEqual(named("status.changed").map((signal) => [signal.properties.state, signal.properties.actor]).reverse(), [["operational", null], ["degraded", null]]);
  assert.deepEqual(named("role.changed").map((signal) => [signal.properties.userId, signal.properties.actor]).reverse(),
    [[sessions.olga.userId, consoleKey.keyId], [sessions.ana.userId, sessions.olga.userId]]);
  assert.deepEqual(named("attachment.published").map((signal) => signal.properties), [{ id: brochureRow.id, actor: sessions.ana.userId }]);
  assert.deepEqual(named("download.opened").map((signal) => signal.properties), [{ fileId: brochure.id, actor: null }]);
  const today = new Date().toISOString().slice(0, 10);
  const usage = await A.owner.call<{ totals: { kind: string; name: string; count: number }[] }>("usage_read", { deploymentId: receipt.id, from: today, to: today });
  const total = (kind: string, name = "") => usage.totals.find((row) => row.kind === kind && row.name === name)?.count ?? 0;
  assert.equal(total("refused", "members.promote"), 2);
  assert.equal(total("refused", "team.post"), 2);
  assert.equal(total("route", "POST /api/updates"), 12);
  assert.equal(total("route", "POST /hooks/status"), 3);
  assert.equal(total("signup"), 3);
  const appReadsSignals = await message(olga.call("signals_read", { deploymentId: receipt.id }));
  refused("Signals", "The in-app owner's session calls signals_read on the runtime endpoint", appReadsSignals);
  check("Signals and Usage", `signals_read returns ${signals.length} Signals. member.joined fires once per person although each joined twice; note.added names its writer; the 12 program updates name the key ${bot.keyId}; status.changed fires for the two deliveries that changed the row with actor null, not for the identical retry; role.changed names the Owner console key for the appointment and the owner for Ana's promotion, and none of the four refused role and team calls recorded one. Usage counts refused members.promote ${total("refused", "members.promote")}, refused team.post ${total("refused", "team.post")}, POST /api/updates ${total("route", "POST /api/updates")}, POST /hooks/status ${total("route", "POST /hooks/status")}, signup ${total("signup")}. The app itself cannot read them: "${appReadsSignals}".`);

  // --------------------------------------------- Action Contract and conform
  const contractDir = path.join(scratch, "contract");
  await mkdir(contractDir);
  for (const name of capsuleFiles) await writeFile(path.join(contractDir, name), sources[name]);
  const [contractCode, contractOut, contractErr] = await cli(["contract", "--dir", contractDir]);
  assert.equal(contractCode, 0, contractOut + contractErr);
  assert.equal(contractErr.trim(), "", "contract warns about nothing");
  const written = JSON.parse(await readFile(path.join(contractDir, "action-contract.json"), "utf8"));
  assert.equal(written.actions.length, 27);
  const queries = written.actions.filter((item: any) => item.output?.kind === "rows");
  assert(queries.every((item: any) => item.output.unnamed.length === 0));
  for (const item of queries) {
    const declared = program.actions[item.name].statement.match(/^SELECT (.*?) FROM /)?.[1];
    assert(item.output.columns.length > 0, `${item.name} names its columns`);
    if (declared && !declared.includes("(")) assert.equal(item.output.columns.length, declared.split(",").length, `${item.name} names every column`);
  }
  const unknownColumns = queries.flatMap((item: any) => item.output.columns.filter((column: any) => column.type === "unknown").map((column: any) => `${item.name}.${column.name}`));
  assert.deepEqual(unknownColumns, ["admin.stats.members", "admin.stats.admins", "admin.stats.notes", "admin.stats.attachments", "admin.stats.published", "admin.stats.updates"]);
  refused("Action Contract", "admin.stats returns six COUNT(*) columns, each aliased", `Each is typed "unknown" with nullable null: ${unknownColumns.join(", ")}`);
  // conform runs every mutation once, so it targets the managed Deployment, not the one that travels.
  const conformCookie = await M.signUp("conform@starter.test", `conform-${crypto.randomUUID()}`);
  const conformKey = await A.owner.call<{ key: string }>("api_keys_create", { deploymentId: managedReceipt.id, name: "Conformance", actions: ["updates.post", "owners.appoint", "status.report"] });
  const inputsFile = path.join(scratch, "conform-inputs.json");
  await writeFile(inputsFile, JSON.stringify({ "status.report": { component: "conform", state: "degraded", detail: "conformance run" } }));
  const contractFile = path.join(contractDir, "action-contract.json");
  const conformRuns: Record<string, string> = {};
  const conformFailures: Record<string, string[]> = {};
  for (const [label, extra] of [
    ["session, key and secret", ["--token", tokenOfCookie(conformCookie), "--key", conformKey.key, "--secret", `status_webhook=${webhookSecret}`]],
    ["key and secret", ["--key", conformKey.key, "--secret", `status_webhook=${webhookSecret}`]],
  ] as const) {
    const [, out, err] = await cli(["conform", "--url", managedReceipt.url, "--contract", contractFile, "--inputs", inputsFile, ...extra]);
    conformRuns[label] = out.trim().split("\n").at(-1)!;
    conformFailures[label] = out.split("\n").filter((line) => line.startsWith("FAIL "));
    assert.match(conformRuns[label], /^\d+ passed, \d+ failed, \d+ skipped/, out + err);
  }
  // The session run gets past every refusal but one: conform's probe that omits a nullable
  // input does not accept the action's guard, which a member always meets on updates.publish.
  const nullableProbe = "FAIL updates.publish: answers without the nullable body. Refused with \"Only team admins can publish an update.\" where the contract makes body optional.";
  assert.deepEqual(conformFailures["session, key and secret"], [nullableProbe]);
  assert.match(conformRuns["session, key and secret"], /^\d+ passed, 1 failed, 0 skipped/);
  assert.deepEqual(conformFailures["key and secret"], []);
  assert.match(conformRuns["key and secret"], /^\d+ passed, 0 failed, \d+ skipped/);
  refused("conform", "conform --token as a member, on updates.publish, a guarded mutation with a nullable body", nullableProbe.slice(5));
  check("Action Contract and conform", `littlebox contract writes 27 actions with no warning, and every query names each column it returns. conform against ${managedReceipt.url}: with a session, a key and the secret "${conformRuns["session, key and secret"]}", the one failure being conform's nullable probe on a guarded mutation; with the key and the secret "${conformRuns["key and secret"]}", the session-only actions skipped.`);

  // ------------------------------------------- package export, import on Kernel B
  const exported = await A.owner.call<{ manifest: any; downloads: Record<string, Transfer> }>("package_export", { deploymentId: receipt.id });
  assert.deepEqual(exported.manifest.signupPolicy, { mode: "open", allowlist: [] });
  assert.equal(exported.manifest.visibility, "public");
  const componentBytes: Record<string, Uint8Array> = {};
  for (const content of exported.manifest.contents) componentBytes[content.name] = await A.owner.download(exported.downloads[content.name]);
  const project = config.project;
  const { signupPolicy: _policy, ...projectManifest } = project.manifest;
  await B.owner.call("project_create", { projectId, name: project.name, slug: project.slug, manifest: projectManifest });
  // Secrets never travel. The sender keeps its one secret, so the destination's owner sets the same value.
  await B.owner.call("secret_set", { projectId, name: "status_webhook", value: webhookSecret });
  const components: Record<string, string> = {};
  for (const content of exported.manifest.contents) components[content.name] = (await B.owner.upload(componentBytes[content.name], projectId, content.name)).transferId;
  const { artifact, files } = await buildLocal(cliDir);
  const importUpload = await B.owner.upload(json({ project: { ...project, manifest: projectManifest, files }, visibility: "public", artifact, initialPackage: { manifest: exported.manifest, components } }), projectId);
  const importPlan = await B.owner.call<{ planId: string }>("release_plan", { transferId: importUpload.transferId });
  const imported = await B.owner.call<Receipt>("release_apply", { planId: importPlan.planId });
  await B.owner.cancel(importUpload.transferId).catch(() => {});
  for (const id of Object.values(components)) await B.owner.cancel(id).catch(() => {});
  const I = app(B, imported);
  assert.equal((await B.owner.call<{ mode: string }>("signup_policy_read", { deploymentId: imported.id })).mode, "open");

  // Every account signs in with its password; the origin's sessions do not travel.
  const anaWithOriginCookie = I.runtime(cookies.ana);
  assert.equal((await anaWithOriginCookie.call<{ role: string }>("auth_session")).role, "anonymous");
  const originSession = await message(action(anaWithOriginCookie, "notes.list", { before: null, q: null }));
  assert.equal(originSession, "authentication-required");
  refused("Import", "Ana's origin session cookie on the destination, calling notes.list", originSession);
  const logins = { ana: await I.login(people.ana, passwords.ana), ben: await I.login(people.ben, passwords.ben), olga: await I.login(people.olga, passwords.olga) };
  assert.deepEqual(Object.values(logins).map((login) => login.status), [200, 200, 200]);
  const anaB = I.runtime(logins.ana.cookie), benB = I.runtime(logins.ben.cookie), olgaB = I.runtime(logins.olga.cookie), visitorB = I.runtime();
  // Rows, roles and guards.
  assert.equal((await action(anaB, "notes.list", { before: null, q: null })).length, 20);
  assert.deepEqual((await action(benB, "notes.list", { before: null, q: null })).map((row: any) => row.title), ["Ben's private note"]);
  assert.equal(await message(action(benB, "notes.remove", { id: notes2[0].id })), program.actions["notes.remove"].guard);
  assert.deepEqual((await action(anaB, "team.roster")).map((row: any) => [row.display_name, row.role]), [["Ana", "admin"], ["olga", "owner"]]);
  assert.deepEqual(await action(benB, "team.posts", { before: null }), []);
  assert.equal(await message(action(benB, "team.post", { body: "Still no" })), program.actions["team.post"].guard);
  assert.equal((await action(olgaB, "members.list", { q: null })).length, 3);
  assert.equal(await message(action(anaB, "me.rename", { displayName: "ben" })), program.actions["me.rename"].guard);
  // Files and their visibility.
  assert.equal((await I.fileGet(avatar.id)).status, 200);
  assert.equal((await I.fileGet(brochure.id)).status, 200);
  assert.equal((await I.fileGet(draft.id)).status, 404);
  assert.equal((await I.fileGet(draft.id, { cookie: logins.ben.cookie })).status, 404);
  assert.equal(await (await I.fileGet(draft.id, { cookie: logins.ana.cookie })).text(), "Draft contract: payment in 30 days.");
  assert.equal((await I.fileGet(photos.id, { cookie: logins.ana.cookie })).status, 404);
  assert.equal(await message(action(visitorB, "notes.list", { before: null, q: null })), "authentication-required");
  assert.equal((await I.route("GET", "/api/updates")).body.result.length, 10);
  // Keys arrive revoked; the destination's owner issues a new one.
  const oldKey = await I.route("POST", "/api/updates", { body: JSON.stringify({ title: "From the origin key" }), bearer: bot.key });
  assert.deepEqual(oldKey, { status: 401, body: { error: "invalid-api-key" } });
  refused("Import", "The origin's Release bot key on the destination", `${oldKey.status} ${oldKey.body.error}`);
  const keysB = (await B.owner.call<{ keys: { name: string; actions: string[]; revokedReason: string | null }[] }>("api_keys_list", { deploymentId: imported.id })).keys;
  assert.deepEqual(keysB.map((key) => [key.name, key.revokedReason]).sort(), [["Owner console", "import"], ["Release bot", "import"]]);
  const botB = await B.owner.call<{ key: string }>("api_keys_create", { deploymentId: imported.id, name: "Release bot", actions: ["updates.post"] });
  assert.equal((await I.route("POST", "/api/updates", { body: JSON.stringify({ title: "Release 1.13" }), bearer: botB.key })).status, 200);
  assert.deepEqual(await I.route("POST", "/api/owners", { body: JSON.stringify({ userId: sessions.ben.userId }), bearer: botB.key }), { status: 403, body: { error: "This API key may not call owners.appoint." } });
  // The origin's last accepted delivery, captured and replayed at the destination, is refused:
  // the memory of signatures travels with the accounts. Past 300 seconds it is stale anyway.
  const lastAge = Math.floor(Date.now() / 1000) - Number(lastSignature.match(/^t=(\d+)/)![1]);
  const originSigned = await I.hook(degraded, lastSignature);
  assert.deepEqual(originSigned, lastAge < 290 ? { status: 409, body: { error: "replayed-signature" } } : { status: 401, body: { error: "stale-timestamp" } });
  refused("Import", `The origin's last accepted webhook delivery, ${lastAge} seconds old, replayed on the destination`, `${originSigned.status} ${originSigned.body.error}`);
  assert.equal((await I.hook(operational, await I.sign(operational))).body.result.changes, 1);
  // Signals and the open, verified signup travel too.
  const signalsB = await B.owner.call<{ signals: unknown[]; nextCursor: string | null }>("signals_read", { deploymentId: imported.id, name: "note.added" });
  assert.equal(signalsB.signals.length, 26);
  const newcomer = await I.auth("/_littlebox/auth/signup", { email: "dee@starter.test" });
  assert.equal(newcomer.status, 202);
  assert.equal(cookieOf(newcomer), undefined);
  check("Package export and import", `package_export carried ${exported.manifest.contents.map((item: any) => `${item.name} (${item.size} B)`).join(", ")}. Kernel B imported it after its own secret_set: Ana, Ben and the owner sign in with their passwords; rows, the admin and owner roles, every guard, the private draft, the public avatar and brochure, the deleted file's 404 and the visitor's limits hold. Both origin keys answer invalid-api-key and appear as revoked at import; a new key works within its scope. The webhook takes the secret the owner set again, the origin's last delivery replayed answers ${originSigned.status} ${originSigned.body.error}, 26 note.added Signals arrived, and a new signup is still open and waits for its link.`);

  // --------------------------------------------- Chrome under the app's policy
  if (process.env.LITTLEBOX_SHOWCASE_BROWSER !== "0") {
    if (shots) await mkdir(shots, { recursive: true });
    const browser = await chromium.launch({ channel: "chrome" });
    const appUrl = new URL(receipt.url);
    try {
      for (const [who, scheme] of [["visitor", "light"], ["ana", "dark"], ["olga", "light"]] as const) {
        const context = await browser.newContext({ colorScheme: scheme, locale: "en-US", viewport: { width: 1100, height: 1000 } });
        if (who !== "visitor") await context.addCookies([{ name: cookies[who].split("=")[0], value: tokenOfCookie(cookies[who]), domain: appUrl.hostname, path: "/" }]);
        const page = await context.newPage();
        const violations: string[] = [];
        page.on("console", (entry) => { if (entry.type() === "error" || /Content Security Policy|Refused to/i.test(entry.text())) violations.push(`${entry.text()} ${entry.location().url}`); });
        page.on("pageerror", (error) => violations.push(`page error: ${error.message}`));
        page.on("response", (answer) => { if (answer.status() >= 400) violations.push(`${answer.status()} ${new URL(answer.url()).pathname}`); });
        await page.addInitScript(() => document.addEventListener("securitypolicyviolation", (event) =>
          console.error(`Refused to load ${event.blockedURI} by ${event.violatedDirective}`)));
        const response = await page.goto(receipt.url, { waitUntil: "load" });
        assert.equal(response?.status(), 200);
        assert(response?.headers()["content-security-policy"]?.includes("img-src 'self'"));
        await page.locator("#updates-list li").first().waitFor({ timeout: 10_000 });
        assert.equal(await page.locator("#status-list li").count(), 1);
        assert.equal(await page.locator("#downloads-list a").first().textContent(), "brochure.txt");
        if (who === "visitor") {
          assert.equal(await page.locator("#sign-in").isVisible(), true);
          assert.equal(await page.locator("#member").isVisible(), false);
          assert.equal(await page.locator("#team").isVisible(), false);
          await page.locator("#updates-more").click();
          await page.waitForFunction(() => document.querySelectorAll("#updates-list li").length === 12);
        } else {
          await page.locator("#notes-list li, #notes-empty:not([hidden])").first().waitFor({ timeout: 10_000 });
          assert.equal(await page.locator("#who-role").textContent(), who === "ana" ? "admin" : "owner");
          assert.equal(await page.locator("#team").isVisible(), true);
          assert.equal(await page.locator("#admin").isVisible(), who === "olga");
          await page.waitForFunction(() => {
            const image = document.querySelector<HTMLImageElement>("#roster img.avatar");
            return image?.complete && image.naturalWidth === 64;
          }, undefined, { timeout: 10_000 });
        }
        if (who === "ana") {
          // The guard's sentence reaches the person, in the page.
          await page.getByLabel("Display name").fill("ben");
          await page.getByRole("button", { name: "Save name" }).click();
          await page.locator("#notice[data-kind='error']").filter({ hasText: program.actions["me.rename"].guard }).waitFor({ timeout: 10_000 });
          await page.getByLabel("Display name").fill("Ana");
        }
        await page.waitForTimeout(300);
        assert.deepEqual(violations, [], `The ${who} page in ${scheme} broke under the Content Security Policy.`);
        await page.evaluate(() => { (document.activeElement as HTMLElement | null)?.blur(); window.scrollTo(0, 0); });
        if (shots) await page.screenshot({ path: path.join(shots, `starter-${who}-${scheme}.png`), fullPage: true });
        await context.close();
      }
    } finally { await browser.close(); }
    check("Browser", `Chrome loads the app as a visitor in light, as Ana (team admin) in dark and as the owner in light, with no policy violation or console error. The visitor pages through all 12 updates; Ana's rename to "ben" shows "${renameTaken}" in the page; the owner sees the admin area and Ana's avatar from the file path${shots ? `. Screenshots in ${shots}` : ""}.`);
  }

  console.log(JSON.stringify({ app: "starter", checks: observations.length, observations, refusals, conform: conformRuns }, null, 2));
} finally {
  for (const kernel of kernels) kernel.kill();
  await Promise.all(kernels.map((kernel) => kernel.exited));
  await rm(scratch, { recursive: true, force: true });
}
