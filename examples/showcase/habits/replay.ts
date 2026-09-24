// Replays the habits showcase against its own Kernel: verified signup, rows
// bound to the actor, an API key on a route, Signals and Usage, Eject from the
// managed path, and a scheduled copy restored into a fresh Kernel. It asserts
// what works and what each primitive refuses, then removes its Kernel and data.
//
//   LITTLEBOX_SHOWCASE_PORT=21410 bun run examples/showcase/habits/replay.ts
import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import initSqlJs from "sql.js";
import { chromium } from "playwright";
import { McpClient } from "../../../src/mcp/client";
import { buildLocal } from "../../../scripts/littlebox";
import { createCapsuleValidator } from "../../../src/agent/gate";

const repo = path.resolve(import.meta.dir, "../../..");
const appFiles = ["index.html", "styles.css", "app.js", "schema.sql", "schema.ts", "actions.json"];
const port = Number(process.env.LITTLEBOX_SHOWCASE_PORT ?? 21410);
assert(Number.isInteger(port) && port >= 1024 && port <= 65535, "Set LITTLEBOX_SHOWCASE_PORT to a free local port.");
const url = `http://127.0.0.1:${port}`;
const scratch = await mkdtemp(path.join(tmpdir(), "littlebox-showcase-habits-"));
const dataDir = path.join(scratch, "data");
const copyDir = path.join(scratch, "second-disk");
const restoredDir = path.join(scratch, "restored");
const screenshotDir = process.env.LITTLEBOX_SHOWCASE_SCREENSHOTS;
const binary = path.join(repo, "kernel-rs/target/debug/littlebox-kernel");
const setupKey = crypto.randomUUID() + crypto.randomUUID();
const ownerEmail = "owner@habits.test";
const ownerPassword = `owner-${crypto.randomUUID()}`;
const anaPassword = `ana-${crypto.randomUUID()}`;
const benPassword = `ben-${crypto.randomUUID()}`;
const projectId = "showcase_habits";
const observations: { scenario: string; result: string }[] = [];
let kernel: ReturnType<typeof Bun.spawn> | undefined;
let address = 0;
const freshAddress = () => `198.18.${Math.floor(++address / 250)}.${(address % 250) + 1}`;

function check(scenario: string, result: string) {
  observations.push({ scenario, result });
  console.log(`PASS ${scenario}. ${result}`);
}

function kernelEnv(data: string, extra: Record<string, string> = {}) {
  const inherited = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_") && !key.startsWith("VITE_")));
  return {
    ...inherited,
    LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_PUBLIC_ORIGIN: url,
    LITTLEBOX_HOST_SUFFIX: "localhost", LITTLEBOX_DATA_DIR: data, LITTLEBOX_ADMIN_KEY: setupKey,
    LITTLEBOX_BUN_PATH: process.execPath, LITTLEBOX_SOURCE_ROOT: repo, LITTLEBOX_TRUSTED_PROXIES: "127.0.0.1",
    LITTLEBOX_ACCOUNT_VERIFICATION: "required", LITTLEBOX_MAIL_TRANSPORT: "development",
    ...extra,
  };
}

async function startKernel(data: string, extra: Record<string, string> = {}) {
  const child = Bun.spawn([binary], { cwd: repo, env: kernelEnv(data, extra), stdout: "ignore", stderr: "inherit" });
  for (let attempt = 0; attempt < 150; attempt++) {
    if (child.exitCode !== null) throw Error(`The Kernel on port ${port} stopped during startup. Is the port free?`);
    try { if ((await fetch(`${url}/_littlebox/health`)).ok) return child; } catch {}
    await Bun.sleep(100);
  }
  child.kill();
  throw Error("The Kernel did not start.");
}

async function stopKernel() {
  if (!kernel) return;
  kernel.kill();
  await kernel.exited;
  kernel = undefined;
}

type Mail = { to: string; purpose: string; link: string };
async function mailbox(data: string): Promise<Mail[]> {
  const text = await readFile(path.join(data, "mailbox.jsonl"), "utf8").catch(() => "");
  return text.split("\n").filter(Boolean).map((line) => JSON.parse(line));
}
/** The mail worker writes the development mailbox after the request returns. */
async function waitForMail(email: string, seen: number, data = dataDir) {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    const messages = (await mailbox(data)).filter((item) => item.to === email);
    if (messages.length > seen) return messages.at(-1)!;
    await Bun.sleep(100);
  }
  throw Error(`No mail reached ${email}.`);
}
const mailCount = async (email: string, data = dataDir) => (await mailbox(data)).filter((item) => item.to === email).length;
const tokenOf = (link: string) => {
  const fragment = new URLSearchParams(new URL(link).hash.slice(1));
  return fragment.get("verification") ?? fragment.get("recovery") ?? fragment.get("invitation")!;
};

async function post(route: string, body: unknown, headers: Record<string, string> = {}) {
  return fetch(`${url}${route}`, {
    method: "POST", redirect: "manual",
    headers: { "content-type": "application/json", "x-forwarded-for": freshAddress(), ...headers },
    body: JSON.stringify(body),
  });
}
const cookieOf = (response: Response) => response.headers.get("set-cookie")?.split(";")[0];

function withHeaders(endpoint: string, headers: Record<string, string>) {
  return new McpClient(endpoint, { fetch: (input, init) => {
    const merged = new Headers(init?.headers);
    for (const [name, value] of Object.entries(headers)) merged.set(name, value);
    merged.set("x-forwarded-for", freshAddress());
    return fetch(input, { ...init, headers: merged });
  } });
}

async function readSources(root: string) {
  return Object.fromEntries(await Promise.all(appFiles.map(async (name) => [name, await readFile(path.join(root, name), "utf8")] as const)));
}

const utcDay = (offset = 0) => new Date(Date.now() + offset * 86_400_000).toISOString().slice(0, 10);

try {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
  assert.equal(await build.exited, 0, "The Kernel must compile.");
  const appRoot = path.join(scratch, "app");
  await mkdir(appRoot);
  for (const name of appFiles) await cp(path.join(import.meta.dir, name), path.join(appRoot, name));
  const sources = await readSources(appRoot);

  // ---------------------------------------------------------------- the Gate
  // The editor's Validation Gate on sql.js, then the Kernel on release_plan.
  const gate = createCapsuleValidator(() => initSqlJs());
  const passed = await gate.validate({ "schema.sql": sources["schema.sql"], "actions.json": sources["actions.json"] } as never);
  assert.equal(passed.ok, true, JSON.stringify(passed.errors));
  const program = JSON.parse(sources["actions.json"]);
  const streakSql: string = program.actions["habits.streaks"].statement;
  // Three programs the doors must refuse: the streak without its LIMIT, a
  // recursive calendar, and a Signal that tries to name the account.
  const variants: Record<string, (actions: any) => void> = {
    unbounded: (actions) => { actions["habits.streaks"].statement = streakSql.replace(/ LIMIT 20$/, ""); },
    recursive: (actions) => { actions["habits.streaks"].statement = "WITH RECURSIVE d(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM d WHERE n < 7) SELECT n AS day_offset FROM d WHERE ?1 IS NOT NULL AND ?2 IS NOT NULL LIMIT 7"; },
    signalActor: (actions) => { actions["checkins.add"].signal.properties.push("user"); },
  };
  const gateRefusals: Record<string, string> = {};
  for (const [name, change] of Object.entries(variants)) {
    const variant = structuredClone(program);
    change(variant.actions);
    const result = await gate.validate({ "schema.sql": sources["schema.sql"], "actions.json": JSON.stringify(variant) } as never);
    assert.equal(result.ok, false);
    gateRefusals[name] = result.errors[0].detail ?? result.errors[0].message;
  }
  assert.match(gateRefusals.unbounded, /uses OVER, which keeps one result row per stored row, and has no LIMIT/);
  assert.match(gateRefusals.recursive, /Runtime query only allows: SELECT/);
  assert.match(gateRefusals.signalActor, /Action checkins.add names signal property user, which is not one of its input bindings\./);

  // ------------------------------------------------------------ the Kernel
  kernel = await startKernel(dataDir, { LITTLEBOX_COPY_DIR: copyDir, LITTLEBOX_COPY_INTERVAL_SECONDS: "2" });

  // Required verification covers the creator too: the setup key answers 202 and a link.
  const ownerSignup = await post("/_littlebox/owner/signup", { email: ownerEmail, setupKey });
  assert.equal(ownerSignup.status, 202, await ownerSignup.clone().text());
  const ownerVerify = await post("/_littlebox/owner/verify", { token: tokenOf((await waitForMail(ownerEmail, 0)).link), password: ownerPassword });
  assert.equal(ownerVerify.status, 200, await ownerVerify.clone().text());
  const ownerCookie = cookieOf(ownerVerify)!;
  const owner = withHeaders(`${url}/mcp`, { cookie: ownerCookie });

  // The Kernel refuses the same two streak variants on release_plan.
  const kernelRefusals: Record<string, string> = {};
  await owner.call("project_create", { projectId: "showcase_habits_probe", name: "Habits probe", slug: "habits-probe", manifest: { version: 1, visibility: "public" } });
  for (const [name, change] of Object.entries(variants)) {
    const variant = structuredClone(program);
    change(variant.actions);
    const payload = { project: { id: "showcase_habits_probe", name: "Habits probe", slug: "habits-probe", manifest: { version: 1, visibility: "public" }, files: { ...sources, "actions.json": JSON.stringify(variant) } }, visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>probe</main>" } } };
    const upload = await owner.upload(new TextEncoder().encode(JSON.stringify(payload)), "showcase_habits_probe");
    try {
      await assert.rejects(owner.call("release_plan", { transferId: upload.transferId }), (error: Error) => { kernelRefusals[name] = error.message; return true; });
    } finally { await owner.cancel(upload.transferId).catch(() => {}); }
  }
  assert.match(kernelRefusals.unbounded, /uses OVER, which keeps one result row per stored row, and has no LIMIT/);
  assert.match(kernelRefusals.recursive, /contains disallowed SQL/);
  assert.equal(kernelRefusals.signalActor, gateRefusals.signalActor);
  assert.equal(kernelRefusals.unbounded, gateRefusals.unbounded);
  check("Streak SQL within the LIMIT rules",
    `The Gate passes the capsule and both doors refuse the streak query without its top-level LIMIT. A WITH RECURSIVE calendar is refused by both, with different words: Gate "${gateRefusals.recursive}", Kernel "${kernelRefusals.recursive}". A Signal property that is not an input key is refused by both with "${kernelRefusals.signalActor}"`);

  // ------------------------------------------------- the editor's managed path
  // project_sources_save and release_plan/apply in the shapes the editor sends.
  const manifest = {
    version: 1, visibility: "public",
    runtime: {
      database: { mode: "declared-actions", actions: Object.entries(program.actions).map(([name, action]: [string, any]) => ({ name, kind: action.kind, access: action.access })) },
      auth: { mode: "optional" }, files: { access: "deployment" },
    },
  };
  const managedSource = (files: Record<string, string>) => ({
    id: projectId, name: "Habits", slug: "habits", files, directories: [],
    databaseModel: { source: files["schema.ts"], acceptedSource: files["schema.ts"], revision: "r1" },
    manifest, updatedAt: Date.now(),
  });
  const created = await owner.call<{ sourceAuthority: string }>("project_create", { projectId, name: "Habits", slug: "habits", manifest, sourceAuthority: "managed" });
  assert.equal(created.sourceAuthority, "managed");
  const saveSources = async (expectedRevision: number, files = sources) => {
    const upload = await owner.upload(new TextEncoder().encode(JSON.stringify(managedSource(files))), projectId);
    try { return await owner.call<{ revision: number }>("project_sources_save", { projectId, transferId: upload.transferId, expectedRevision }); }
    finally { await owner.cancel(upload.transferId).catch(() => {}); }
  };
  assert.deepEqual(await saveSources(0), { revision: 1 });
  const { artifact } = await buildLocal(appRoot);
  const releasePayload = { project: managedSource(sources), visibility: "public", artifact, initialSignupPolicy: { mode: "open", allowlist: [] } };
  const releaseUpload = await owner.upload(new TextEncoder().encode(JSON.stringify(releasePayload)), projectId);
  let receipt: { id: string; hostname: string; url: string };
  try {
    const plan = await owner.call<{ planId: string }>("release_plan", { transferId: releaseUpload.transferId });
    receipt = await owner.call("release_apply", { planId: plan.planId });
  } finally { await owner.cancel(releaseUpload.transferId).catch(() => {}); }
  const host = receipt.hostname;
  const appOrigin = new URL(receipt.url).origin;
  assert.equal((await owner.call<{ mode: string }>("signup_policy_read", { deploymentId: receipt.id })).mode, "open");
  check("Managed publish", `Saved sources reach revision 1 and the editor-shaped release serves ${receipt.url} as a public app with Open signup.`);

  const appPost = (route: string, body: unknown, headers: Record<string, string> = {}) => post(route, body, { host, ...headers });
  const runtime = (cookie?: string) => withHeaders(`${url}/_littlebox/mcp`, cookie ? { host, cookie } : { host });
  const action = <T = any>(client: McpClient, name: string, input: object = {}) => client.call<T>("actions_invoke", { name, input });
  const sql = <T = any>(statement: string) => owner.call<{ rows: T[] }>("database_execute", { deploymentId: receipt.id, operation: "sql", input: { sql: statement } });
  const appUsers = async () => (await owner.call<{ users: { userId: string; email: string; state: string; verified: boolean }[] }>("app_users_list", { deploymentId: receipt.id })).users;

  // -------------------------------------------- Open signup, verification required
  const ana = "ana@habits.test";
  const first = await appPost("/_littlebox/auth/signup", { email: ana });
  assert.equal(first.status, 202, await first.clone().text());
  assert.equal(cookieOf(first), undefined, "A signup must not open a session before verification.");
  const firstLink = await waitForMail(ana, 0);
  assert.equal(firstLink.purpose, "verification");
  const pendingLogin = await appPost("/_littlebox/auth/login", { email: ana, password: anaPassword });
  const pendingLoginBody = await pendingLogin.text();
  assert.equal(pendingLogin.status, 401, pendingLoginBody);
  assert.equal(cookieOf(pendingLogin), undefined);
  assert.equal((await runtime().call<{ role: string }>("auth_session")).role, "anonymous");
  await assert.rejects(action(runtime(), "habits.list"), /authentication-required/);
  const pendingUsers = await appUsers();
  assert.deepEqual(pendingUsers.map((user) => [user.email, user.state, user.verified]), [[ana, "pending", false]]);

  // A password sent with a required-mode signup does not survive.
  const ben = "ben@habits.test";
  const benSignup = await appPost("/_littlebox/auth/signup", { email: ben, password: "typed-at-signup-1234" });
  const benSignupStatus = benSignup.status;
  assert.equal(cookieOf(benSignup), undefined);
  const benTypedLogin = await appPost("/_littlebox/auth/login", { email: ben, password: "typed-at-signup-1234" });
  assert.equal(benTypedLogin.status, 401, await benTypedLogin.clone().text());

  // A second signup for the pending address keeps its ID and replaces the link.
  const second = await appPost("/_littlebox/auth/signup", { email: ana });
  assert.equal(second.status, 202);
  assert.equal(cookieOf(second), undefined);
  const secondLink = await waitForMail(ana, 1);
  const stale = await appPost("/_littlebox/auth/verify", { token: tokenOf(firstLink.link), password: anaPassword });
  assert.equal(stale.status, 410, await stale.clone().text());
  const verified = await appPost("/_littlebox/auth/verify", { token: tokenOf(secondLink.link), password: anaPassword });
  assert.equal(verified.status, 200, await verified.clone().text());
  const anaPhone = runtime(cookieOf(verified)!);
  const anaPhoneCookie = cookieOf(verified)!;
  const anaSession = await anaPhone.call<{ userId: string; name: string; role: string }>("auth_session");
  assert.equal(anaSession.name, ana);
  assert.equal(anaSession.userId, pendingUsers[0].userId, "Re-signing up a pending address must keep its account ID.");

  // A third signup, now for the active address, gives the caller nothing.
  const mailBefore = await mailCount(ana);
  const steal = await appPost("/_littlebox/auth/signup", { email: ana });
  assert.equal(steal.status, 202);
  assert.equal(cookieOf(steal), undefined);
  await Bun.sleep(1_500);
  const mailToActive = (await mailbox(dataDir)).filter((item) => item.to === ana).slice(mailBefore).map((item) => item.purpose);
  const stolenLogin = await appPost("/_littlebox/auth/login", { email: ana, password: "attacker-password-1234" });
  assert.equal(stolenLogin.status, 401, await stolenLogin.clone().text());
  assert.equal((await anaPhone.call<{ userId: string }>("auth_session")).userId, anaSession.userId);
  const laptopLogin = await appPost("/_littlebox/auth/login", { email: ana, password: anaPassword });
  assert.equal(laptopLogin.status, 200);
  const anaLaptop = runtime(cookieOf(laptopLogin)!);
  const benVerify = await appPost("/_littlebox/auth/verify", { token: tokenOf((await waitForMail(ben, 0)).link), password: benPassword });
  assert.equal(benVerify.status, 200);
  const benClient = runtime(cookieOf(benVerify)!);
  const users = await appUsers();
  assert.deepEqual(users.map((user) => [user.email, user.state, user.verified]), [[ana, "active", true], [ben, "active", true]]);
  check("Verified signup",
    `Signup answers 202 with no cookie; the pending account's login answers ${pendingLogin.status} ${pendingLoginBody}; a re-signup keeps user ID ${anaSession.userId} and turns the first link into 410; a signup for the active address answers 202, mails [${mailToActive.join(", ") || "nothing"}], and a guessed password answers 401 while both of Ana's sessions keep working. A password sent with a required-mode signup (status ${benSignupStatus}) does not sign in.`);

  // ------------------------------------------------ two devices, one account
  const today = utcDay();
  assert.equal((await action(anaPhone, "habits.create", { name: "Read 20 pages" })).changes, 1);
  assert.equal((await action(anaLaptop, "habits.create", { name: "Walk" })).changes, 1);
  const [read, walk] = await action<{ id: number; name: string }[]>(anaLaptop, "habits.list");
  assert.deepEqual([read.name, walk.name], ["Read 20 pages", "Walk"]);
  assert.deepEqual(await action(anaPhone, "habits.list"), await action(anaLaptop, "habits.list"));
  let completions = 0;
  for (const offset of [-6, -3, -2, -1]) {
    assert.equal((await action(anaPhone, "checkins.add", { habitId: read.id, day: utcDay(offset), device: "phone" })).changes, 1);
    completions++;
  }
  assert.equal((await action(anaLaptop, "checkins.add", { habitId: read.id, day: today, device: "computer" })).changes, 1);
  assert.equal((await action(anaLaptop, "checkins.add", { habitId: walk.id, day: today, device: "computer" })).changes, 1);
  completions += 2;
  // The same check from the second device the same day is absorbed by the UNIQUE key.
  assert.equal((await action(anaPhone, "checkins.add", { habitId: read.id, day: today, device: "phone" })).changes, 0);
  // Only the last 30 days and tomorrow are accepted.
  assert.equal((await action(anaPhone, "checkins.add", { habitId: read.id, day: utcDay(-45), device: "phone" })).changes, 0);
  await assert.rejects(action(anaPhone, "checkins.add", { habitId: read.id, day: "2026-11-061", device: "phone" }), /day is too long/);
  const streaks = await action<{ habit_id: number; current_streak: number; best_streak: number; total_days: number }[]>(anaLaptop, "habits.streaks", { today });
  assert.deepEqual(streaks, [
    { habit_id: read.id, current_streak: 4, best_streak: 4, total_days: 5 },
    { habit_id: walk.id, current_streak: 1, best_streak: 1, total_days: 1 },
  ]);
  const week = await action<{ habit_id: number; day: string; device: string }[]>(anaPhone, "checkins.since", { from: utcDay(-6) });
  assert.equal(week.length, 6);
  assert.deepEqual([...new Set(week.map((row) => row.device))].sort(), ["computer", "phone"]);
  check("Two devices", `Ana's phone and laptop sessions share one account: habits from both, check-ins from both, and the streak query returns current 4, best 4, total 5 for the gapped habit. A second check-in for the same day changes 0 rows.`);

  // ------------------------------------------------- rows bound to the actor
  assert.deepEqual(await action(benClient, "habits.list"), []);
  assert.deepEqual(await action(benClient, "checkins.since", { from: utcDay(-30) }), []);
  assert.deepEqual(await action(benClient, "habits.streaks", { today }), []);
  assert.equal((await action(benClient, "habits.archive", { id: read.id })).changes, 0);
  assert.equal((await action(benClient, "checkins.add", { habitId: read.id, day: utcDay(-4), device: "laptop" })).changes, 0);
  assert.equal((await action(benClient, "checkins.remove", { habitId: read.id, day: today })).changes, 0);
  await assert.rejects(action(benClient, "habits.list", { userId: anaSession.userId }), /Unexpected action input: userId/);
  assert.equal((await action(benClient, "habits.create", { name: "Ben's habit" })).changes, 1);
  assert.deepEqual((await action(anaPhone, "habits.list")).map((row: any) => row.name), ["Read 20 pages", "Walk"]);
  assert.equal((await action(anaPhone, "checkins.since", { from: utcDay(-30) })).length, 6);
  check("Rows bound to the actor", "Ben reads no habit, check-in or streak of Ana's; archive, check-in and uncheck on her habit change 0 rows; a userId input is refused with \"Unexpected action input: userId\"; Ana's rows are unchanged.");

  // ------------------------------------------- an API key on an actor-bound route
  const key = await owner.call<{ key: string; keyId: string }>("api_keys_create", { deploymentId: receipt.id, name: "Phone shortcut", actions: ["habits.import", "habits.mine", "checkins.log"] });
  const route = (method: "GET" | "POST", routePath: string, body?: unknown, headers: Record<string, string> = {}) => fetch(`${url}${routePath}`, {
    method, headers: { host, "x-forwarded-for": freshAddress(), ...(body ? { "content-type": "application/json" } : {}), ...headers },
    body: body ? JSON.stringify(body) : undefined,
  }).then(async (response) => ({ status: response.status, body: await response.json() as any }));
  const bearer = { authorization: `Bearer ${key.key}` };
  assert.deepEqual(await route("POST", "/api/habits", { name: "Stretch" }, bearer), { status: 200, body: { result: { changes: 1, lastInsertId: 4 } } });
  const keyRows = (await sql<{ user_id: string }>("SELECT user_id FROM habits WHERE name = 'Stretch'")).rows;
  assert.deepEqual(keyRows.map((row) => row.user_id), [key.keyId]);
  assert(key.keyId.startsWith("key_"));
  const mine = await route("GET", "/api/habits", undefined, bearer);
  assert.deepEqual(mine.body.result.map((row: any) => row.name), ["Stretch"]);
  const keyForAna = await route("POST", "/api/checkins", { habitId: read.id, day: utcDay(-4), device: "shortcut" }, bearer);
  assert.equal(keyForAna.status, 200);
  assert.equal(keyForAna.body.result.changes, 0);
  const keyOwnCheckin = await route("POST", "/api/checkins", { habitId: 4, day: today, device: "shortcut" }, bearer);
  assert.equal(keyOwnCheckin.body.result.changes, 1);
  completions++;
  const visitorRoute = await route("POST", "/api/habits", { name: "Nobody" });
  assert.equal(visitorRoute.status, 400);
  assert.equal(visitorRoute.body.error, "This action requires an authenticated actor.");
  // The same route with Ana's cookie from the app's own origin runs as Ana.
  const anaRoute = await route("POST", "/api/checkins", { habitId: read.id, day: utcDay(-4), device: "route" }, { host: new URL(receipt.url).host, cookie: anaPhoneCookie, origin: appOrigin });
  assert.equal(anaRoute.body.result?.changes, 1, JSON.stringify(anaRoute));
  completions++;
  const foreignOrigin = await route("POST", "/api/checkins", { habitId: walk.id, day: utcDay(-1), device: "route" }, { host: new URL(receipt.url).host, cookie: anaPhoneCookie, origin: `http://other-app.localhost:${port}` });
  assert.equal(foreignOrigin.status, 400, JSON.stringify(foreignOrigin));
  assert.equal(foreignOrigin.body.error, "This action requires an authenticated actor.");
  // A hand-written client whose Host lacks the port its Origin carries runs as a visitor too.
  const portless = await route("POST", "/api/checkins", { habitId: walk.id, day: utcDay(-1), device: "route" }, { cookie: anaPhoneCookie, origin: appOrigin });
  assert.deepEqual(portless, { status: 400, body: { error: "This action requires an authenticated actor." } });
  assert.deepEqual((await action(anaPhone, "habits.list")).map((row: any) => row.name), ["Read 20 pages", "Walk"]);
  check("API key on an actor-bound route",
    `A key writes a habit whose owner is the key's ID ${key.keyId}; GET /api/habits with the key returns only that row; the key's check-in on Ana's habit changes 0 rows; a visitor gets 400 "${visitorRoute.body.error}". Ana's cookie from the app origin runs the route as Ana; from another origin it runs as a visitor (${foreignOrigin.status}).`);

  // --------------------------------------------------------- Signals and Usage
  const signals: { name: string; at: number; properties: Record<string, unknown> }[] = [];
  for (let cursor: string | null | undefined; ;) {
    const page = await owner.call<{ signals: typeof signals; nextCursor: string | null }>("signals_read", { deploymentId: receipt.id, name: "habit.completed", ...(cursor ? { cursor } : {}) });
    signals.push(...page.signals);
    if (!page.nextCursor) break;
    cursor = page.nextCursor;
  }
  // Every call that ran its statement records one, whether or not a row changed.
  const zeroChangeSignals = signals.length - completions;
  assert.equal(signals.length, 12, JSON.stringify(signals.map((s) => s.properties)));
  assert(signals.some((signal) => signal.properties.day === utcDay(-45)), "A check-in outside the window that changed nothing still records its Signal.");
  assert(signals.every((signal) => Object.keys(signal.properties).sort().join() === "day,device,habitId"));
  const usage = await owner.call<{ days: { day: string; kind: string; name: string; count: number }[]; totals: { kind: string; name: string; count: number }[] }>("usage_read", { deploymentId: receipt.id, from: utcDay(-1), to: utcDay(1) });
  const total = (kind: string, name = "") => usage.totals.find((row) => row.kind === kind && row.name === name)?.count ?? 0;
  assert.equal(total("action", "checkins.add"), 9);
  assert.equal(total("route", "POST /api/checkins"), 3);
  assert.equal(total("route", "POST /api/habits"), 1);
  assert.equal(total("signup"), 2);
  // The app's own session cannot read either store.
  let runtimeUsage = "";
  await assert.rejects(anaPhone.call("usage_read", { deploymentId: receipt.id, from: utcDay(-1), to: utcDay(1) }), (error: Error) => { runtimeUsage = error.message; return true; });
  await assert.rejects(anaPhone.call("signals_read", { deploymentId: receipt.id }), (error: Error) => error.message === runtimeUsage);
  const dailyCheckins = usage.days.filter((row) => row.kind === "action" && row.name === "checkins.add");
  check("Signals and Usage",
    `signals_read returns ${signals.length} habit.completed Signals with habitId, day and device; ${completions} of the calls added a row, ${zeroChangeSignals} changed nothing and still recorded one, including Ben's attempt on Ana's habit and a key's. usage_read gives checkins.add ${total("action", "checkins.add")} on ${dailyCheckins.map((row) => row.day).join(", ")}, POST /api/checkins ${total("route", "POST /api/checkins")}, signup ${total("signup")}, login ${total("login")}, session ${total("session")}. An app session calling usage_read or signals_read gets "${runtimeUsage}".`);

  // --------------------------------------------------- Action Contract and conform
  const conformEmail = "conform@habits.test";
  assert.equal((await appPost("/_littlebox/auth/signup", { email: conformEmail })).status, 202);
  const conformVerify = await appPost("/_littlebox/auth/verify", { token: tokenOf((await waitForMail(conformEmail, 0)).link), password: `conform-${crypto.randomUUID()}` });
  assert.equal(conformVerify.status, 200);
  const conformToken = cookieOf(conformVerify)!.split("=").slice(1).join("=");
  const contractDir = path.join(scratch, "contract");
  await mkdir(contractDir);
  for (const name of appFiles) await cp(path.join(appRoot, name), path.join(contractDir, name));
  const cli = (args: string[], env: Record<string, string> = {}) => {
    const child = Bun.spawn(["bun", "run", path.join(repo, "scripts/littlebox.ts"), ...args], {
      cwd: repo, stdout: "pipe", stderr: "pipe",
      env: { ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("LITTLEBOX_"))), ...env },
    });
    return Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  };
  const [contractCode, contractOut, contractErr] = await cli(["contract", "--dir", contractDir]);
  assert.equal(contractCode, 0, contractOut + contractErr);
  const contract = JSON.parse(await readFile(path.join(contractDir, "action-contract.json"), "utf8"));
  assert.equal(contract.actions.length, 10);
  const [conformCode, conformOut, conformErr] = await cli(["conform", "--url", receipt.url, "--contract", path.join(contractDir, "action-contract.json"), "--token", conformToken, "--key", key.key]);
  const conformTotals = conformOut.trim().split("\n").at(-1)!;
  assert.equal(conformCode, 0, conformOut + conformErr);
  // conform skips a route that binds the caller's ID even when it holds a key; the replay checked those three routes with the key above.
  const conformSkipped = conformOut.split("\n").filter((line) => line.startsWith("SKIP"));
  assert.match(conformTotals, /^\d+ passed, 0 failed, 3 skipped/);
  assert.deepEqual(conformSkipped.map((line) => line.split(":")[0]).sort(), ["SKIP checkins.log", "SKIP habits.import", "SKIP habits.mine"]);
  assert(conformSkipped.every((line) => line.endsWith("Binds the caller's ID; a route caller with an ID needs an API key or a session cookie.")));
  check("Action Contract", `littlebox contract writes 10 actions${contractErr.trim() ? ` with warnings: ${contractErr.trim()}` : " with no warning"}; littlebox conform reports "${conformTotals}" and skips the three actor-bound routes although it was given an API key.`);

  // ------------------------------------------------------------------ Eject
  const lifecycle = await owner.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish", "manage_lifecycle"], projectIds: [projectId], ttlSeconds: 3600 });
  const ejectedDir = path.join(scratch, "ejected");
  await mkdir(ejectedDir);
  const [ejectCode, ejectOut, ejectErr] = await cli(["eject", "--dir", ejectedDir, "--project", projectId, "--url", url], { LITTLEBOX_MCP_TOKEN: lifecycle.token });
  assert.equal(ejectCode, 0, ejectOut + ejectErr);
  const ejectedFiles = (await readdir(ejectedDir)).filter((name) => !name.startsWith(".")).sort();
  const ejectedConfig = JSON.parse(await readFile(path.join(ejectedDir, "littlebox.json"), "utf8"));
  assert.equal(ejectedConfig.url, url);
  assert.equal(ejectedConfig.project.sourceAuthority, "external");
  const indexHtml = await readFile(path.join(ejectedDir, "index.html"), "utf8");
  assert(indexHtml.includes("Build 1"));
  await writeFile(path.join(ejectedDir, "index.html"), indexHtml.replace("Build 1", "Build 2, published from the ejected folder"));
  const [publishCode, publishOut, publishErr] = await cli(["publish", "--dir", ejectedDir], { LITTLEBOX_MCP_TOKEN: lifecycle.token });
  assert.equal(publishCode, 0, publishOut + publishErr);
  assert.equal(publishOut.trim(), receipt.url);
  const shell = await (await fetch(url, { headers: { host } })).text();
  assert(shell.includes("Build 2, published from the ejected folder"));
  assert.equal((await anaPhone.call<{ userId: string }>("auth_session")).userId, anaSession.userId, "Eject keeps sessions.");
  assert.equal((await action(anaLaptop, "habits.streaks", { today }))[0].total_days, 6);
  assert.equal((await appPost("/_littlebox/auth/login", { email: ben, password: benPassword })).status, 200);
  let managedRefusal = "";
  await assert.rejects(saveSources(1), (error: Error) => { managedRefusal = error.message; return true; });
  assert.match(managedRefusal, /^This project was ejected: its sources are maintained outside Littlebox/);
  const frozen = await owner.call<{ readOnly: boolean; revision: number; transfer: { transferId: string } }>("project_sources_read", { projectId });
  assert.equal(frozen.readOnly, true);
  await owner.cancel(frozen.transfer.transferId).catch(() => {});
  check("Eject and publish from outside",
    `littlebox eject writes ${ejectedFiles.join(", ")} with sourceAuthority external; littlebox publish serves "Build 2" at the same URL; Ana's phone session, her rows and Ben's password survive; project_sources_save answers "${managedRefusal}"`);

  // ------------------------------------------------------- copy and restore
  const written = Date.now();
  let copies: { targets: { lastAttemptAt: number | null; lastSuccessAt: number | null; lastCopyId: string | null; lastError: string | null }[]; alert: string | null } | undefined;
  for (let attempt = 0; attempt < 300; attempt++) {
    copies = (await (await fetch(`${url}/_littlebox/health`)).json()).copies;
    if (copies!.targets.length > 0 && copies!.targets.every((target) => target.lastAttemptAt! > written && target.lastSuccessAt! >= target.lastAttemptAt!)) break;
    await Bun.sleep(100);
  }
  assert.equal(copies!.alert, null);
  assert(copies!.targets.every((target) => target.lastAttemptAt! > written && target.lastSuccessAt! >= target.lastAttemptAt!), JSON.stringify(copies));
  const signalCountBefore = (await owner.call<{ signals: unknown[]; nextCursor: string | null }>("signals_read", { deploymentId: receipt.id, name: "habit.completed" })).signals.length;
  await stopKernel();
  const restore = Bun.spawn([binary, "restore", "--from", copyDir], { cwd: repo, env: kernelEnv(restoredDir), stdout: "pipe", stderr: "pipe" });
  const [restoreCode, restoreOut, restoreErr] = await Promise.all([restore.exited, new Response(restore.stdout).text(), new Response(restore.stderr).text()]);
  assert.equal(restoreCode, 0, restoreOut + restoreErr);
  assert.match(restoreOut + restoreErr, /checksums verified/);
  kernel = await startKernel(restoredDir);

  assert.equal((await anaPhone.call<{ role: string }>("auth_session")).role, "anonymous", "A restore signs app users out.");
  await assert.rejects(owner.call("projects_list"), (error: Error) => /sign in|unauthor|401/i.test(error.message));
  const ownerAgain = await post("/_littlebox/owner/login", { email: ownerEmail, password: ownerPassword });
  assert.equal(ownerAgain.status, 200);
  const restoredOwner = withHeaders(`${url}/mcp`, { cookie: cookieOf(ownerAgain)! });
  await assert.rejects(new McpClient(`${url}/mcp`, { token: lifecycle.token }).call("projects_list"));
  const anaAgain = await appPost("/_littlebox/auth/login", { email: ana, password: anaPassword });
  assert.equal(anaAgain.status, 200);
  const anaRestored = runtime(cookieOf(anaAgain)!);
  assert.deepEqual((await action(anaRestored, "habits.list")).map((row: any) => row.name), ["Read 20 pages", "Walk"]);
  assert.deepEqual((await action(anaRestored, "habits.streaks", { today })).map((row: any) => [row.current_streak, row.total_days]), [[5, 6], [1, 1]]);
  const keyAfter = await route("GET", "/api/habits", undefined, bearer);
  assert.equal(keyAfter.status, 200);
  assert.deepEqual(keyAfter.body.result.map((row: any) => row.name), ["Stretch"]);
  const restoredSignals = await restoredOwner.call<{ signals: unknown[]; nextCursor: string | null }>("signals_read", { deploymentId: receipt.id, name: "habit.completed" });
  assert.equal(restoredSignals.signals.length, signalCountBefore);
  let restoredRefusal = "";
  const restoredUpload = await restoredOwner.upload(new TextEncoder().encode(JSON.stringify(managedSource(sources))), projectId);
  try {
    await assert.rejects(restoredOwner.call("project_sources_save", { projectId, transferId: restoredUpload.transferId, expectedRevision: 1 }), (error: Error) => { restoredRefusal = error.message; return true; });
  } finally { await restoredOwner.cancel(restoredUpload.transferId).catch(() => {}); }
  assert.equal(restoredRefusal, managedRefusal);
  assert((await (await fetch(url, { headers: { host } })).text()).includes("Build 2"));
  check("Copy and restore",
    `A directory copy taken ${copies!.targets[0].lastCopyId} restores into an empty data directory. The restored Kernel serves Build 2, signs out Ana's sessions, the owner's session and the delegated token, keeps the passwords, rows, streaks, ${signalCountBefore} Signals, the ejected state and the API key, which still reads GET /api/habits.`);

  // ------------------------------------------- Chrome under the app's policy
  if (process.env.LITTLEBOX_SHOWCASE_BROWSER !== "0") {
    const [name, ...value] = cookieOf(anaAgain)!.split("=");
    const browser = await chromium.launch({ channel: "chrome" });
    try {
      for (const [scheme, signedIn] of [["light", false], ["light", true], ["dark", true]] as const) {
        const context = await browser.newContext({ colorScheme: scheme, viewport: { width: 720, height: 900 } });
        if (signedIn) await context.addCookies([{ name, value: value.join("="), url: receipt.url }]);
        const page = await context.newPage();
        const violations: string[] = [];
        page.on("console", (message) => { if (/Content Security Policy|Refused to/i.test(message.text())) violations.push(message.text()); });
        page.on("pageerror", (error) => violations.push(`page error: ${error.message}`));
        await page.addInitScript(() => document.addEventListener("securitypolicyviolation", (event) =>
          console.error(`Refused to load ${event.blockedURI} by ${event.violatedDirective}`)));
        const response = await page.goto(receipt.url, { waitUntil: "load" });
        assert.equal(response?.status(), 200);
        assert(response?.headers()["content-security-policy"]?.includes("frame-ancestors 'none'"));
        if (signedIn) {
          await page.locator(".habit").first().waitFor({ timeout: 10_000 });
          assert.equal(await page.locator(".habit").count(), 2);
          assert.equal(await page.locator("#gate").isVisible(), false);
          assert.equal(await page.locator("#empty").isVisible(), false);
          assert.equal(await page.locator(".habit .streak .live strong").first().textContent(), "5");
        } else {
          await page.locator("#gate").waitFor({ state: "visible", timeout: 10_000 });
          assert.equal(await page.locator("#app").isVisible(), false);
        }
        await page.waitForTimeout(500);
        assert.deepEqual(violations, [], `The ${scheme} page broke under the Content Security Policy.`);
        if (signedIn && screenshotDir) {
          await mkdir(screenshotDir, { recursive: true });
          await page.screenshot({ path: path.join(screenshotDir, `habits-${scheme}.png`), fullPage: true });
        }
        await context.close();
      }
    } finally { await browser.close(); }
    check("Content Security Policy", `The signed-out gate and Ana's signed-in page load in Chrome with no policy violation or page error, in light and dark.${screenshotDir ? ` Screenshots in ${screenshotDir}.` : ""}`);
  }

  console.log(JSON.stringify({ app: "habits", checks: observations.length, observations }, null, 2));
} finally {
  await stopKernel();
  await rm(scratch, { recursive: true, force: true });
}
