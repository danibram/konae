// Replays the habits showcase against its own Kernel: verified signup, rows
// bound to the actor, a day closed by a schedule, an API key on a route,
// Signals and Usage, a publish from a folder, and a scheduled copy
// restored into a fresh Kernel. It asserts
// what works and what each primitive refuses, then removes its Kernel and data.
//
//   KONAE_SHOWCASE_PORT=21410 bun run examples/showcase/habits/replay.ts
import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import initSqlJs from "sql.js";
import { chromium } from "playwright";
import { McpClient } from "../../../src/mcp/client";
import { appDir, buildLocal } from "../../../scripts/konae";
import { KERNEL_VERSION, type AppSettings } from "../../../src/lib/app-folder";
import { createCapsuleValidator } from "../../../src/agent/gate";

const repo = path.resolve(import.meta.dir, "../../..");
const appFiles = ["index.html", "styles.css", "app.js", "schema.sql", "actions.json"];
const port = Number(process.env.KONAE_SHOWCASE_PORT ?? 21410);
assert(Number.isInteger(port) && port >= 1024 && port <= 65535, "Set KONAE_SHOWCASE_PORT to a free local port.");
const url = `http://127.0.0.1:${port}`;
const scratch = await mkdtemp(path.join(tmpdir(), "konae-showcase-habits-"));
const dataDir = path.join(scratch, "data");
const copyDir = path.join(scratch, "second-disk");
const restoredDir = path.join(scratch, "restored");
const screenshotDir = process.env.KONAE_SHOWCASE_SCREENSHOTS;
const binary = path.join(repo, "kernel-rs/target/debug/konae");
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
  const inherited = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_")));
  return {
    ...inherited,
    KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_PUBLIC_ORIGIN: url,
    KONAE_HOST_SUFFIX: "localhost", KONAE_DATA_DIR: data, KONAE_ADMIN_KEY: setupKey,
    KONAE_TRUSTED_PROXIES: "127.0.0.1",
    KONAE_ACCOUNT_VERIFICATION: "required", KONAE_MAIL_TRANSPORT: "development",
    // days.close is declared daily at 00:05 UTC; the test clock runs it every 2 seconds.
    KONAE_SCHEDULE_MIN_SECONDS: "2",
    ...extra,
  };
}

async function startKernel(data: string, extra: Record<string, string> = {}) {
  const child = Bun.spawn([binary], { cwd: repo, env: kernelEnv(data, extra), stdout: "ignore", stderr: "inherit" });
  for (let attempt = 0; attempt < 150; attempt++) {
    if (child.exitCode !== null) throw Error(`The Kernel on port ${port} stopped during startup. Is the port free?`);
    try { if ((await fetch(`${url}/_konae/health`)).ok) return child; } catch {}
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
  return Object.fromEntries(await Promise.all(appFiles.map(async (name) => [name, await readFile(path.join(appDir(root), name), "utf8")] as const)));
}

const utcDay = (offset = 0) => new Date(Date.now() + offset * 86_400_000).toISOString().slice(0, 10);

try {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
  assert.equal(await build.exited, 0, "The Kernel must compile.");
  // An app folder whose app/ holds this example's sources.
  const appRoot = path.join(scratch, "habits");
  await mkdir(appDir(appRoot), { recursive: true });
  for (const name of appFiles) await cp(path.join(import.meta.dir, name), path.join(appDir(appRoot), name));
  const sources = await readSources(appRoot);

  // ---------------------------------------------------------------- the Gate
  // The editor's Validation Gate on sql.js, then the Kernel on release_plan.
  const gate = createCapsuleValidator(() => initSqlJs());
  const passed = await gate.validate({ "schema.sql": sources["schema.sql"], "actions.json": sources["actions.json"] } as never);
  assert.equal(passed.ok, true, JSON.stringify(passed.errors));
  const program = JSON.parse(sources["actions.json"]);
  const streakSql: string = program.actions["habits.streaks"].statement;
  // Three programs the doors must refuse: the streak without its LIMIT, a
  // recursive calendar, and a Signal that names an input it does not have.
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
  assert.equal(gateRefusals.recursive, "Action habits.streaks is a query, so it must start with SELECT. Move the WITH clause into a subquery, as in SELECT ... FROM (WITH ... SELECT ...).");
  assert.match(gateRefusals.signalActor, /Action checkins.add names signal property user, which is not one of its input bindings\./);

  // ------------------------------------------------------------ the Kernel
  kernel = await startKernel(dataDir, { KONAE_COPY_DIR: copyDir, KONAE_COPY_INTERVAL_SECONDS: "2" });

  // Required verification covers the creator too: the setup key answers 202 and a link.
  const ownerSignup = await post("/_konae/owner/signup", { email: ownerEmail, setupKey });
  assert.equal(ownerSignup.status, 202, await ownerSignup.clone().text());
  const ownerVerify = await post("/_konae/owner/verify", { token: tokenOf((await waitForMail(ownerEmail, 0)).link), password: ownerPassword });
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
  assert.equal(kernelRefusals.recursive, gateRefusals.recursive);
  assert.equal(kernelRefusals.signalActor, gateRefusals.signalActor);
  assert.equal(kernelRefusals.unbounded, gateRefusals.unbounded);
  check("Streak SQL within the LIMIT rules",
    `The Gate passes the capsule and both doors refuse the streak query without its top-level LIMIT. A WITH RECURSIVE calendar that leads the statement is refused by both with "${kernelRefusals.recursive}". A Signal property that is neither an input key nor actor is refused by both with "${kernelRefusals.signalActor}"; the capsule's Signals list actor and pass`);

  // ---------------------------------------------------------- the editor's path
  // project_sources_save and release_plan/apply in the shapes the editor sends.
  const manifest = {
    version: 1, visibility: "public",
    runtime: {
      database: { mode: "declared-actions", actions: Object.entries(program.actions).map(([name, action]: [string, any]) => ({ name, kind: action.kind, access: action.access })) },
      auth: { mode: "optional" }, files: { access: "deployment" },
    },
  };
  const editorSource = (files: Record<string, string>) => ({
    id: projectId, name: "Habits", slug: "habits", files, directories: [],
    manifest, updatedAt: Date.now(),
  });
  await owner.call("project_create", { projectId, name: "Habits", slug: "habits", manifest });
  const saveSources = async (expectedRevision: number, files = sources) => {
    const upload = await owner.upload(new TextEncoder().encode(JSON.stringify(editorSource(files))), projectId);
    try { return await owner.call<{ revision: number }>("project_sources_save", { projectId, transferId: upload.transferId, expectedRevision }); }
    finally { await owner.cancel(upload.transferId).catch(() => {}); }
  };
  assert.deepEqual(await saveSources(0), { revision: 1 });
  const { artifact } = await buildLocal(appRoot);
  const releasePayload = { project: editorSource(sources), visibility: "public", artifact, initialSignupPolicy: { mode: "open", allowlist: [] } };
  const releaseUpload = await owner.upload(new TextEncoder().encode(JSON.stringify(releasePayload)), projectId);
  let receipt: { id: string; hostname: string; url: string };
  try {
    const plan = await owner.call<{ planId: string }>("release_plan", { transferId: releaseUpload.transferId });
    receipt = await owner.call("release_apply", { planId: plan.planId });
  } finally { await owner.cancel(releaseUpload.transferId).catch(() => {}); }
  const host = receipt.hostname;
  const appOrigin = new URL(receipt.url).origin;
  assert.equal((await owner.call<{ mode: string }>("signup_policy_read", { deploymentId: receipt.id })).mode, "open");
  check("Editor publish", `Saved sources reach revision 1 and the editor-shaped release serves ${receipt.url} as a public app with Open signup.`);

  const appPost = (route: string, body: unknown, headers: Record<string, string> = {}) => post(route, body, { host, ...headers });
  const runtime = (cookie?: string) => withHeaders(`${url}/_konae/mcp`, cookie ? { host, cookie } : { host });
  const action = <T = any>(client: McpClient, name: string, input: object = {}) => client.call<T>("actions_invoke", { name, input });
  const sql = <T = any>(statement: string) => owner.call<{ rows: T[] }>("database_execute", { deploymentId: receipt.id, operation: "sql", input: { sql: statement } });
  const appUsers = async () => (await owner.call<{ users: { userId: string; email: string; state: string; verified: boolean }[] }>("app_users_list", { deploymentId: receipt.id })).users;

  // -------------------------------------------- Open signup, verification required
  const ana = "ana@habits.test";
  const first = await appPost("/_konae/auth/signup", { email: ana });
  assert.equal(first.status, 202, await first.clone().text());
  // A signup opens no session. It sets only the signup hint, so a sign-in from this browser before the link can say why it failed.
  const signupHint = cookieOf(first);
  assert.match(signupHint ?? "", /^konae_signup=/, "A signup must not open a session before verification.");
  const firstLink = await waitForMail(ana, 0);
  assert.equal(firstLink.purpose, "verification");
  const pendingLogin = await appPost("/_konae/auth/login", { email: ana, password: anaPassword }, { cookie: signupHint! });
  const pendingLoginBody = await pendingLogin.text();
  assert.equal(pendingLogin.status, 401, pendingLoginBody);
  assert.equal(JSON.parse(pendingLoginBody).error, "Email or password is incorrect. If you signed up with this email in this browser, open the link we emailed to choose your password first.");
  assert.equal(cookieOf(pendingLogin), undefined);
  const otherBrowserLogin = await appPost("/_konae/auth/login", { email: ana, password: anaPassword });
  assert.deepEqual([otherBrowserLogin.status, await otherBrowserLogin.json()], [401, { error: "Email or password is incorrect." }]);
  assert.equal((await runtime().call<{ role: string }>("auth_session")).role, "anonymous");
  await assert.rejects(action(runtime(), "habits.list"), /authentication-required/);
  const pendingUsers = await appUsers();
  assert.deepEqual(pendingUsers.map((user) => [user.email, user.state, user.verified]), [[ana, "pending", false]]);

  // A password sent with a required-mode signup does not survive.
  const ben = "ben@habits.test";
  const benSignup = await appPost("/_konae/auth/signup", { email: ben, password: "typed-at-signup-1234" });
  const benSignupStatus = benSignup.status;
  assert.match(cookieOf(benSignup) ?? "", /^konae_signup=/);
  const benTypedLogin = await appPost("/_konae/auth/login", { email: ben, password: "typed-at-signup-1234" });
  assert.equal(benTypedLogin.status, 401, await benTypedLogin.clone().text());

  // A second signup for the pending address keeps its ID and replaces the link.
  const second = await appPost("/_konae/auth/signup", { email: ana });
  assert.equal(second.status, 202);
  assert.equal(cookieOf(second), signupHint, "The same email gets the same hint cookie, and no session.");
  const secondLink = await waitForMail(ana, 1);
  const stale = await appPost("/_konae/auth/verify", { token: tokenOf(firstLink.link), password: anaPassword });
  assert.equal(stale.status, 410, await stale.clone().text());
  const verified = await appPost("/_konae/auth/verify", { token: tokenOf(secondLink.link), password: anaPassword });
  assert.equal(verified.status, 200, await verified.clone().text());
  const anaPhone = runtime(cookieOf(verified)!);
  const anaPhoneCookie = cookieOf(verified)!;
  const anaSession = await anaPhone.call<{ userId: string; name: string; role: string }>("auth_session");
  assert.equal(anaSession.name, ana);
  assert.equal(anaSession.userId, pendingUsers[0].userId, "Re-signing up a pending address must keep its account ID.");

  // A third signup, now for the active address, gives the caller nothing.
  const mailBefore = await mailCount(ana);
  const steal = await appPost("/_konae/auth/signup", { email: ana });
  assert.equal(steal.status, 202);
  assert.equal(cookieOf(steal), signupHint, "An active address gets the hint cookie a pending one gets, so the cookie tells nothing.");
  await Bun.sleep(1_500);
  const mailToActive = (await mailbox(dataDir)).filter((item) => item.to === ana).slice(mailBefore).map((item) => item.purpose);
  const stolenLogin = await appPost("/_konae/auth/login", { email: ana, password: "attacker-password-1234" });
  assert.equal(stolenLogin.status, 401, await stolenLogin.clone().text());
  assert.equal((await anaPhone.call<{ userId: string }>("auth_session")).userId, anaSession.userId);
  const laptopLogin = await appPost("/_konae/auth/login", { email: ana, password: anaPassword });
  assert.equal(laptopLogin.status, 200);
  const anaLaptop = runtime(cookieOf(laptopLogin)!);
  const benVerify = await appPost("/_konae/auth/verify", { token: tokenOf((await waitForMail(ben, 0)).link), password: benPassword });
  assert.equal(benVerify.status, 200);
  const benClient = runtime(cookieOf(benVerify)!);
  const users = await appUsers();
  assert.deepEqual(users.map((user) => [user.email, user.state, user.verified]), [[ana, "active", true], [ben, "active", true]]);
  check("Verified signup",
    `Signup answers 202 with no session cookie, only the signup hint; the pending account's login from that browser answers ${pendingLogin.status} ${pendingLoginBody}, and from another browser the plain 401; a re-signup keeps user ID ${anaSession.userId} and turns the first link into 410; a signup for the active address answers 202, mails [${mailToActive.join(", ") || "nothing"}], and a guessed password answers 401 while both of Ana's sessions keep working. A password sent with a required-mode signup (status ${benSignupStatus}) does not sign in.`);

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
  await assert.rejects(action(benClient, "habits.archive", { id: read.id }), /This habit is already archived, or it is not yours\./);
  assert.equal((await action(benClient, "checkins.add", { habitId: read.id, day: utcDay(-4), device: "laptop" })).changes, 0);
  assert.equal((await action(benClient, "checkins.remove", { habitId: read.id, day: today })).changes, 0);
  await assert.rejects(action(benClient, "habits.list", { userId: anaSession.userId }), /Unexpected action input: userId/);
  assert.equal((await action(benClient, "habits.create", { name: "Ben's habit" })).changes, 1);
  assert.deepEqual((await action(anaPhone, "habits.list")).map((row: any) => row.name), ["Read 20 pages", "Walk"]);
  assert.equal((await action(anaPhone, "checkins.since", { from: utcDay(-30) })).length, 6);
  check("Rows bound to the actor", "Ben reads no habit, check-in or streak of Ana's; archive, check-in and uncheck on her habit change 0 rows; a userId input is refused with \"Unexpected action input: userId\"; Ana's rows are unchanged. The archive is refused with its guard, \"This habit is already archived, or it is not yours.\"");

  // ---------------------------------------------------------- close the day
  // days.close has "schedule": { "daily": "00:05" }, so the Kernel runs it with nobody
  // calling. A habit started today does not count against yesterday, and the replay made
  // Ana's habits a moment ago, so the owner dates them a week back, as if she started then.
  await sql(`UPDATE habits SET created_at = created_at - ${7 * 86_400_000} WHERE user_id = '${anaSession.userId}'`);
  const yesterday = utcDay(-1);
  type Day = { day: string; habits: number; done: number };
  let closedDays: Day[] = [];
  for (const deadline = Date.now() + 20_000; closedDays.length === 0; await Bun.sleep(250)) {
    assert(Date.now() < deadline, "days.close did not close yesterday within 20 seconds.");
    closedDays = await action<Day[]>(anaPhone, "days.recent");
  }
  assert.deepEqual(closedDays, [{ day: yesterday, habits: 2, done: 1 }]);
  assert.deepEqual(await action(benClient, "days.recent"), [], "Ben's habit started today, so he had nothing to close yesterday.");
  // A scheduled mutation with no route refuses every caller, so only the timer closes a day.
  const closedByBen = await action(benClient, "days.close").then(() => "answered", (error: Error) => error.message);
  assert.equal(closedByBen, "Action days.close runs only on its schedule.");
  await Bun.sleep(2500);
  type Run = { action: string; at: number; outcome: string; changes: number };
  const closeRuns = (await owner.call<{ scheduled: Run[] }>("usage_read", { deploymentId: receipt.id, from: utcDay(-1), to: utcDay(1) })).scheduled;
  assert(closeRuns.length >= 2 && closeRuns.every((run) => run.action === "days.close" && run.outcome === "ok"), JSON.stringify(closeRuns));
  assert.equal(closeRuns.filter((run) => run.changes > 0).length, 1, "Later runs find yesterday already closed.");
  assert.deepEqual(await action(anaPhone, "days.recent"), closedDays);
  check("Close the day", `days.close, declared daily at 00:05 UTC and run here every 2 seconds, closes ${yesterday} for Ana as ${JSON.stringify(closedDays[0])} with nobody calling; Ben, whose habit started today, gets no row. Ben calling days.close himself is refused with "${closedByBen}". usage_read lists ${closeRuns.length} runs, all ok, and only the first changed a row: the others found the day closed.`);

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
  // The three routes declare "callers": ["key"], so a visitor and a signed-in person are
  // refused before the action runs, and an app session checks in through actions_invoke.
  const visitorRoute = await route("POST", "/api/habits", { name: "Nobody" });
  assert.deepEqual(visitorRoute, { status: 401, body: { error: "authentication-required" } });
  const visitorRead = await route("GET", "/api/habits");
  assert.deepEqual(visitorRead, { status: 401, body: { error: "authentication-required" } });
  const anaRoute = await route("POST", "/api/checkins", { habitId: read.id, day: utcDay(-4), device: "route" }, { host: new URL(receipt.url).host, cookie: anaPhoneCookie, origin: appOrigin });
  assert.deepEqual(anaRoute, { status: 401, body: { error: "authentication-required" } });
  assert.equal((await action(anaPhone, "checkins.add", { habitId: read.id, day: utcDay(-4), device: "phone" })).changes, 1);
  completions++;
  assert.deepEqual((await action(anaPhone, "habits.list")).map((row: any) => row.name), ["Read 20 pages", "Walk"]);
  check("API key on an actor-bound route",
    `A key writes a habit whose owner is the key's ID ${key.keyId}; GET /api/habits with the key returns only that row; the key's check-in on Ana's habit changes 0 rows. The routes take only a key: a visitor gets ${visitorRoute.status} "${visitorRoute.body.error}" on POST and GET, and Ana's cookie from the app origin gets ${anaRoute.status} too.`);

  // --------------------------------------------------------- Signals and Usage
  const signals: { name: string; at: number; properties: Record<string, unknown> }[] = [];
  for (let cursor: string | null | undefined; ;) {
    const page = await owner.call<{ signals: typeof signals; nextCursor: string | null }>("signals_read", { deploymentId: receipt.id, name: "habit.completed", ...(cursor ? { cursor } : {}) });
    signals.push(...page.signals);
    if (!page.nextCursor) break;
    cursor = page.nextCursor;
  }
  // Only a call that added a row records one, and it names who checked in.
  assert.equal(signals.length, completions, JSON.stringify(signals.map((s) => s.properties)));
  assert(!signals.some((signal) => signal.properties.day === utcDay(-45)), "A check-in outside the window changed nothing and records no Signal.");
  assert(signals.every((signal) => Object.keys(signal.properties).sort().join() === "actor,day,device,habitId"));
  const actors = signals.map((signal) => signal.properties.actor);
  assert.equal(actors.filter((actor) => actor === anaSession.userId).length, completions - 1, "Ana's check-ins name her.");
  assert.equal(actors.filter((actor) => actor === key.keyId).length, 1, "The key's check-in names the key.");
  const usage = await owner.call<{ days: { day: string; kind: string; name: string; count: number }[]; totals: { kind: string; name: string; count: number }[] }>("usage_read", { deploymentId: receipt.id, from: utcDay(-1), to: utcDay(1) });
  const total = (kind: string, name = "") => usage.totals.find((row) => row.kind === kind && row.name === name)?.count ?? 0;
  assert.equal(total("action", "checkins.add"), 10);
  assert.equal(total("route", "POST /api/checkins"), 2);
  assert.equal(total("route", "POST /api/habits"), 1);
  assert.equal(total("signup"), 2);
  // The app's own session cannot read either store.
  let runtimeUsage = "";
  await assert.rejects(anaPhone.call("usage_read", { deploymentId: receipt.id, from: utcDay(-1), to: utcDay(1) }), (error: Error) => { runtimeUsage = error.message; return true; });
  await assert.rejects(anaPhone.call("signals_read", { deploymentId: receipt.id }), (error: Error) => error.message === runtimeUsage);
  const dailyCheckins = usage.days.filter((row) => row.kind === "action" && row.name === "checkins.add");
  check("Signals and Usage",
    `signals_read returns ${signals.length} habit.completed Signals with habitId, day, device and actor, one for each of the ${completions} calls that added a row; the calls that changed nothing, Ben's attempt on Ana's habit and a key's among them, record none. ${completions - 1} name Ana and 1 names the key. usage_read gives checkins.add ${total("action", "checkins.add")} on ${dailyCheckins.map((row) => row.day).join(", ")}, POST /api/checkins ${total("route", "POST /api/checkins")}, signup ${total("signup")}, login ${total("login")}, session ${total("session")}. An app session calling usage_read or signals_read gets "${runtimeUsage}".`);

  // --------------------------------------------------- Action Contract and conform
  const conformEmail = "conform@habits.test";
  assert.equal((await appPost("/_konae/auth/signup", { email: conformEmail })).status, 202);
  const conformVerify = await appPost("/_konae/auth/verify", { token: tokenOf((await waitForMail(conformEmail, 0)).link), password: `conform-${crypto.randomUUID()}` });
  assert.equal(conformVerify.status, 200);
  const conformToken = cookieOf(conformVerify)!.split("=").slice(1).join("=");
  const contractDir = path.join(scratch, "contract");
  await mkdir(appDir(contractDir), { recursive: true });
  for (const name of appFiles) await cp(path.join(appDir(appRoot), name), path.join(appDir(contractDir), name));
  const cli = (args: string[], env: Record<string, string> = {}) => {
    const child = Bun.spawn(["bun", "run", path.join(repo, "scripts/konae.ts"), ...args], {
      cwd: repo, stdout: "pipe", stderr: "pipe",
      env: { ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KONAE_"))), ...env },
    });
    return Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  };
  const [contractCode, contractOut, contractErr] = await cli(["contract", "--dir", contractDir]);
  assert.equal(contractCode, 0, contractOut + contractErr);
  const contract = JSON.parse(await readFile(path.join(contractDir, "action-contract.json"), "utf8"));
  assert.equal(contract.actions.length, 12);
  const [conformCode, conformOut, conformErr] = await cli(["conform", "--url", receipt.url, "--contract", path.join(contractDir, "action-contract.json"), "--token", conformToken, "--key", key.key]);
  const conformTotals = conformOut.trim().split("\n").at(-1)!;
  assert.equal(conformCode, 0, conformOut + conformErr);
  // conform sends the key to the three routes that bind the caller's ID.
  // habits.create and habits.import guard an INSERT under a limit of 20; conform's second
  // identical call inserts again, so it cannot reach the guard. The Chrome pass proves it.
  assert.match(conformTotals, /^\d+ passed, 0 failed, 2 skipped/);
  const conformSkips = conformOut.split("\n").filter((line) => line.startsWith("SKIP")).map((line) => line.slice(0, line.indexOf(". "))).sort();
  assert.deepEqual(conformSkips, ["SKIP habits.create: refuses with its guard when no row changes", "SKIP habits.import: refuses with its guard when no row changes"], conformOut);
  for (const routeName of ["checkins.log", "habits.import", "habits.mine"]) {
    assert(conformOut.split("\n").some((line) => line.startsWith(`PASS ${routeName}: answers`) && line.includes("with an API key")), `${routeName} with the key:\n${conformOut}`);
  }
  check("Action Contract", `konae contract writes 12 actions${contractErr.trim() ? ` with warnings: ${contractErr.trim()}` : " with no warning"}; konae conform reports "${conformTotals}" and calls the three actor-bound routes with the API key.`);

  // ------------------------------------------------- publish from a folder
  // The same project, published with the CLI from a folder that holds its files.
  const publisher = await owner.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish"], projectIds: [projectId], ttlSeconds: 3600 });
  const folder = path.join(scratch, "folder");
  await mkdir(appDir(folder), { recursive: true });
  for (const name of appFiles) await writeFile(path.join(appDir(folder), name), sources[name]);
  const { visibility: _, ...folderManifest } = manifest;
  const settings: AppSettings = {
    id: projectId, name: "Habits", slug: "habits", visibility: "public", signupPolicy: { mode: "open", allowlist: [] },
    signInMethods: ["password"], kernel: KERNEL_VERSION, manifest: folderManifest,
  };
  await writeFile(path.join(folder, "konae.json"), JSON.stringify(settings, null, 2));
  const indexHtml = await readFile(path.join(appDir(folder), "index.html"), "utf8");
  assert(indexHtml.includes("Build 1"));
  await writeFile(path.join(appDir(folder), "index.html"), indexHtml.replace("Build 1", "Build 2, published from a folder"));
  const [publishCode, publishOut, publishErr] = await cli(["publish", "--dir", folder, "--url", url], { KONAE_MCP_TOKEN: publisher.token });
  assert.equal(publishCode, 0, publishOut + publishErr);
  assert.equal(publishOut.trim(), receipt.url);
  const shell = await (await fetch(url, { headers: { host } })).text();
  assert(shell.includes("Build 2, published from a folder"));
  assert.equal((await anaPhone.call<{ userId: string }>("auth_session")).userId, anaSession.userId, "Publishing from a folder keeps sessions.");
  assert.equal((await action(anaLaptop, "habits.streaks", { today }))[0].total_days, 6);
  assert.equal((await appPost("/_konae/auth/login", { email: ben, password: benPassword })).status, 200);
  assert.deepEqual(await saveSources(1), { revision: 2 });
  check("Publish from a folder",
    `konae publish serves "Build 2" at the same URL; Ana's phone session, her rows and Ben's password survive, and the editor still saves the project's sources.`);

  // ------------------------------------------------------- copy and restore
  const written = Date.now();
  let copies: { targets: { lastAttemptAt: number | null; lastSuccessAt: number | null; lastCopyId: string | null; lastError: string | null }[]; alert: string | null } | undefined;
  for (let attempt = 0; attempt < 300; attempt++) {
    copies = (await (await fetch(`${url}/_konae/health`)).json()).copies;
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
  const ownerAgain = await post("/_konae/owner/login", { email: ownerEmail, password: ownerPassword });
  assert.equal(ownerAgain.status, 200);
  const restoredOwner = withHeaders(`${url}/mcp`, { cookie: cookieOf(ownerAgain)! });
  await assert.rejects(new McpClient(`${url}/mcp`, { token: publisher.token }).call("projects_list"));
  const anaAgain = await appPost("/_konae/auth/login", { email: ana, password: anaPassword });
  assert.equal(anaAgain.status, 200);
  const anaRestored = runtime(cookieOf(anaAgain)!);
  assert.deepEqual((await action(anaRestored, "habits.list")).map((row: any) => row.name), ["Read 20 pages", "Walk"]);
  assert.deepEqual((await action(anaRestored, "habits.streaks", { today })).map((row: any) => [row.current_streak, row.total_days]), [[5, 6], [1, 1]]);
  const keyAfter = await route("GET", "/api/habits", undefined, bearer);
  assert.equal(keyAfter.status, 200);
  // conform added two habits with the key, its valid call and the same call again to reach
  // habits.import's guard, so the key reads Stretch and those two.
  assert(keyAfter.body.result.map((row: any) => row.name).includes("Stretch"), JSON.stringify(keyAfter.body));
  assert.equal(keyAfter.body.result.length, 3);
  const restoredSignals = await restoredOwner.call<{ signals: unknown[]; nextCursor: string | null }>("signals_read", { deploymentId: receipt.id, name: "habit.completed" });
  assert.equal(restoredSignals.signals.length, signalCountBefore);
  // The closed day and the runs travel in the copy, with the slot each run marked done.
  assert.deepEqual(await action(anaRestored, "days.recent"), closedDays);
  const restoredRuns = (await restoredOwner.call<{ scheduled: Run[] }>("usage_read", { deploymentId: receipt.id, from: utcDay(-1), to: utcDay(1) })).scheduled;
  assert(restoredRuns.length >= closeRuns.length, `${restoredRuns.length} runs after the restore, ${closeRuns.length} before the copy.`);
  assert((await (await fetch(url, { headers: { host } })).text()).includes("Build 2"));
  check("Copy and restore",
    `A directory copy taken ${copies!.targets[0].lastCopyId} restores into an empty data directory. The restored Kernel serves Build 2, signs out Ana's sessions, the owner's session and the delegated token, keeps the passwords, rows, streaks, ${signalCountBefore} Signals, the closed day and ${restoredRuns.length} scheduled runs and the API key, which still reads GET /api/habits.`);

  // ------------------------------------------- Chrome under the app's policy
  if (process.env.KONAE_SHOWCASE_BROWSER !== "0") {
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
      // A double press adds one habit, and the twenty-first is refused with the guard's sentence.
      const context = await browser.newContext({ viewport: { width: 720, height: 900 } });
      await context.addCookies([{ name, value: value.join("="), url: receipt.url }]);
      for (let i = 0; i < 15; i++) await action(anaRestored, "habits.create", { name: `Habit ${i + 3}` });
      const page = await context.newPage();
      const pageErrors: string[] = [];
      page.on("pageerror", (error) => pageErrors.push(error.message));
      await page.goto(receipt.url, { waitUntil: "load" });
      await page.locator(".habit").nth(16).waitFor({ timeout: 10_000 });
      await page.locator("#name").fill("Floss");
      await page.getByRole("button", { name: "Add habit" }).dblclick();
      await page.locator("#status").filter({ hasText: "Added Floss." }).waitFor({ timeout: 10_000 });
      assert.equal((await action<any[]>(anaRestored, "habits.list")).filter((row) => row.name === "Floss").length, 1, "A double press adds one habit.");
      for (const extra of ["Habit 18", "Habit 19"]) await action(anaRestored, "habits.create", { name: extra });
      await page.locator("#name").fill("Stretch");
      await page.getByRole("button", { name: "Add habit" }).click();
      await page.locator("#status.error").filter({ hasText: "You already track 20 habits. Archive one first." }).waitFor({ timeout: 10_000 });
      assert.equal((await action<any[]>(anaRestored, "habits.list")).length, 20);
      assert.deepEqual(pageErrors, []);
      await context.close();
    } finally { await browser.close(); }
    check("Content Security Policy", `The signed-out gate and Ana's signed-in page load in Chrome with no policy violation or page error, in light and dark. A double press of Add habit adds one habit, and the twenty-first is refused on the page with the guard's sentence.${screenshotDir ? ` Screenshots in ${screenshotDir}.` : ""}`);
  }

  console.log(JSON.stringify({ app: "habits", checks: observations.length, observations }, null, 2));
} finally {
  await stopKernel();
  await rm(scratch, { recursive: true, force: true });
}
