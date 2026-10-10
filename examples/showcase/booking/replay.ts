// Replays the booking showcase against its own Kernel: publish, the signup
// policy, the public and staff views, the one-statement booking, a race between
// two staff members, the signed check-in route, the scheduled release of
// bookings nobody checked in, the Action Contract and a Chrome pass under the
// Content Security Policy. Exits 0 when every check holds.
import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import initSqlJs from "sql.js";
import { chromium } from "playwright";
import { McpClient } from "../../../src/mcp/client";
import { signRoute } from "../../../src/lib/route-signature";
import { buildLocal, publishLocal } from "../../../scripts/littlebox";
import { kernelTarget } from "../../../scripts/conform";
import { createCapsuleValidator } from "../../../src/agent/gate";
import { generateActionContract } from "../../../src/lib/action-contract";
import { formatConformanceReport, runConformance } from "../../../src/lib/action-conformance";

const repo = path.resolve(import.meta.dir, "../../..");
const source = import.meta.dir;
const port = Number(process.env.LITTLEBOX_SHOWCASE_PORT ?? 19470);
assert(Number.isInteger(port) && port >= 1024 && port <= 65535, "Set LITTLEBOX_SHOWCASE_PORT to a free local port.");
const shots = process.env.LITTLEBOX_SHOWCASE_SCREENSHOTS;
const url = `http://127.0.0.1:${port}`;
const scratch = await mkdtemp(path.join(tmpdir(), "littlebox-showcase-booking-"));
const setupKey = crypto.randomUUID();
const password = crypto.randomUUID();
const panelSecret = crypto.randomUUID();
const findings: { scenario: string; result: string }[] = [];
let kernel: ReturnType<typeof Bun.spawn> | undefined;
let ownerCookie = "";
let address = 0;
const freshAddress = () => `198.18.${Math.floor(++address / 250)}.${(address % 250) + 1}`;
const check = (scenario: string, result: string) => { findings.push({ scenario, result }); console.log(`PASS ${scenario}. ${result}`); };
const message = (error: unknown) => error instanceof Error ? error.message : String(error);
async function refusal(promise: Promise<unknown>): Promise<string> {
  try { await promise; } catch (error) { return message(error); }
  throw Error("Expected a refusal.");
}
// Days in UTC, the clock the Kernel's `now` binding reads.
const utcDay = (offset: number) => new Date(Date.now() + offset * 86_400_000).toISOString().slice(0, 10);

try {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
  assert.equal(await build.exited, 0, "The Kernel must compile.");
  const taken = await fetch(`${url}/_littlebox/health`).then(() => true, () => false);
  assert(!taken, `Port ${port} already answers. Set LITTLEBOX_SHOWCASE_PORT to a free port.`);
  kernel = Bun.spawn([path.join(repo, "kernel-rs/target/debug/littlebox-kernel")], {
    cwd: repo,
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_") && !key.startsWith("VITE_"))),
      LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_PUBLIC_ORIGIN: url,
      LITTLEBOX_HOST_SUFFIX: "localhost", LITTLEBOX_DATA_DIR: path.join(scratch, "data"),
      LITTLEBOX_ADMIN_KEY: setupKey, LITTLEBOX_BUN_PATH: process.execPath, LITTLEBOX_SOURCE_ROOT: repo,
      LITTLEBOX_TRUSTED_PROXIES: "127.0.0.1",
      // bookings.release is declared every 15 minutes; the test clock runs it every 2 seconds.
      LITTLEBOX_SCHEDULE_MIN_SECONDS: "2",
    },
    stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 100 && !ready; attempt++) {
    assert.equal(kernel.exitCode, null, "The Kernel stopped before startup. Is the port taken?");
    try { ready = (await fetch(`${url}/_littlebox/health`)).ok; } catch { await Bun.sleep(100); }
  }
  assert(ready, "The Kernel did not start.");
  await Bun.sleep(200);
  assert.equal(kernel.exitCode, null, "The Kernel exited after startup. Another process may hold the port.");

  const ownerSignup = await fetch(`${url}/_littlebox/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@office.example", password, setupKey }),
  });
  assert.equal(ownerSignup.status, 200, await ownerSignup.clone().text());
  ownerCookie = ownerSignup.headers.get("set-cookie")!.split(";")[0];
  const owner = new McpClient(`${url}/mcp`, { fetch: ownerFetch });

  // 1. The editor's Validation Gate and the Kernel judge the same Capsule.
  const capsule = { ...(await buildLocal(await copyOf("gate"))).files };
  const gate = createCapsuleValidator(() => initSqlJs());
  const verdict = await gate.validate(capsule);
  assert.deepEqual(verdict.errors, [], "The Validation Gate must pass the booking Capsule.");
  const actionsSource = JSON.parse(capsule["actions.json"]);
  const variants: Record<string, Record<string, string>> = {
    "a trigger that writes the booking log": { "schema.sql": `${capsule["schema.sql"]}\nCREATE TABLE IF NOT EXISTS booking_log (id INTEGER PRIMARY KEY, booking_id INTEGER, at INTEGER);\nCREATE TRIGGER IF NOT EXISTS bookings_logged AFTER INSERT ON bookings BEGIN INSERT INTO booking_log (booking_id, at) VALUES (new.id, new.created_at); END;` },
    "a mutation that starts with WITH": { "actions.json": JSON.stringify({ ...actionsSource, actions: { ...actionsSource.actions, "bookings.create": { ...actionsSource.actions["bookings.create"], statement: actionsSource.actions["bookings.create"].statement.replace(/^INSERT INTO bookings \(([^)]*)\)\n(WITH RECURSIVE span\(slot\) AS \([^\n]*\))\n/, "$2\nINSERT INTO bookings ($1)\n") } } }) },
    "two statements in one action": { "actions.json": JSON.stringify({ ...actionsSource, actions: { ...actionsSource.actions, "bookings.cancel": { ...actionsSource.actions["bookings.cancel"], statement: "UPDATE bookings SET cancelled_at = ?3 WHERE booking_ref = ?1 AND user_id = ?2 AND cancelled_at IS NULL; INSERT INTO staff (user_id, display) VALUES (?2, 'cancelled')" } } }) },
    "a view of free hours": { "schema.sql": `${capsule["schema.sql"]}\nCREATE VIEW IF NOT EXISTS taken AS SELECT room_id, day, slot FROM bookings WHERE cancelled_at IS NULL;` },
  };
  assert.notEqual(variants["a mutation that starts with WITH"]["actions.json"], capsule["actions.json"]);
  await owner.call("project_create", { projectId: "showcase_booking_probe", name: "Booking probe", slug: "booking-probe", manifest: { version: 1, visibility: "public" } });
  // The probe holds its own value: the Kernel refuses one owner's second project with the same one.
  await owner.call("secret_set", { projectId: "showcase_booking_probe", name: "panel_hook", value: `probe_${panelSecret}` });
  const refusals: Record<string, { gate: string; kernel: string }> = {};
  for (const [name, change] of Object.entries(variants)) {
    const files = { ...capsule, ...change };
    const judged = await gate.validate(files);
    assert(!judged.ok, `The gate accepted ${name}.`);
    const kernelSaid = await refusal(publish("showcase_booking_probe", "booking-probe", files));
    refusals[name] = { gate: judged.errors.map((error) => [error.message, (error as any).detail].filter(Boolean).join(" ")).join(" "), kernel: kernelSaid };
    // One rule, one sentence: the Gate's refusal for actions.json is its detail, for schema.sql its message.
    const [first] = judged.errors;
    assert.equal(first.stage === "actions" ? (first as any).detail : first.message, kernelSaid, `${name}: the Gate and the Kernel must give the same sentence.`);
  }
  check("Validation Gate and Kernel", `Both pass the Capsule and both refuse ${Object.keys(refusals).join(", ")} with the same sentence. ${JSON.stringify(refusals)}`);

  // 2. Publish public with `littlebox publish`, with the Allowlist from
  // littlebox.json installed before the first request.
  await owner.call("project_create", { projectId: "showcase_booking", name: "Room board", slug: "booking", manifest: { version: 1, visibility: "public" } });
  const unset = await refusal(publishWithCli("showcase_booking", "booking"));
  assert.match(unset, /signs with the secret panel_hook, which is not set/);
  await owner.call("secret_set", { projectId: "showcase_booking", name: "panel_hook", value: panelSecret });
  const receipt = await publishWithCli("showcase_booking", "booking", { mode: "allowlist", allowlist: ["@office.example"] });
  const deploymentId = receipt.id;
  const policy = await owner.call<{ mode: string; allowlist: string[] }>("signup_policy_read", { deploymentId });
  assert.equal(policy.mode, "allowlist");
  assert.deepEqual(policy.allowlist, ["@office.example"]);
  const sql = (statement: string) => owner.call<any>("database_execute", { deploymentId, operation: "sql", input: { sql: statement } });
  // schema.sql may only create tables and indexes, so the owner adds the rooms.
  await sql("INSERT INTO rooms (id, name, seats) VALUES ('oak', 'Oak', 6), ('birch', 'Birch', 4), ('cedar', 'Cedar', 10), ('pine', 'Pine', 2)");
  check("Publish", `Refused first with "${unset}"; published public with littlebox publish at ${receipt.url} after secret_set, with littlebox.json's signupPolicy allowlist @office.example installed on the first release. The owner seeds rooms with database_execute.`);

  // 3. Allowlist by domain, and its refusal next to Closed's.
  const host = receipt.hostname;
  const signup = async (email: string) => {
    const response = await fetch(`${url}/_littlebox/auth/signup`, {
      method: "POST", headers: { host, "content-type": "application/json", "x-forwarded-for": freshAddress() },
      body: JSON.stringify({ email, password }),
    });
    return { status: response.status, body: await response.json(), cookie: response.headers.get("set-cookie")?.split(";")[0] };
  };
  const anaSignup = await signup("ana@office.example");
  const benSignup = await signup("ben@office.example");
  assert.equal(anaSignup.status, 200, JSON.stringify(anaSignup.body));
  assert.equal(benSignup.status, 200, JSON.stringify(benSignup.body));
  const outsider = await signup("pat@elsewhere.example");
  const subdomain = await signup("sam@sub.office.example");
  await owner.call("signup_policy_set", { deploymentId, mode: "closed" });
  const closed = await signup("cara@office.example");
  const loginWhileClosed = await fetch(`${url}/_littlebox/auth/login`, {
    method: "POST", headers: { host, "content-type": "application/json", "x-forwarded-for": freshAddress() },
    body: JSON.stringify({ email: "ana@office.example", password }),
  });
  await owner.call("signup_policy_set", { deploymentId, mode: "allowlist", allowlist: ["@office.example"] });
  assert.equal(outsider.status, 403);
  assert.deepEqual(outsider, closed, "An address outside the domain must get the same answer as Closed.");
  assert.deepEqual(subdomain, closed, "A subdomain must not match a domain entry.");
  assert.equal(loginWhileClosed.status, 200, "Closed must not sign out existing staff.");
  check("Allowlist signup", `@office.example signs up. pat@elsewhere.example, sam@sub.office.example and, under Closed, cara@office.example all get ${closed.status} ${JSON.stringify(closed.body)}. Existing staff still sign in under Closed.`);

  const runtime = (cookie?: string, bearer?: string) => new McpClient(`${url}/_littlebox/mcp`, { token: bearer, fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", host); headers.set("x-forwarded-for", freshAddress());
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  const call = <T = any>(client: McpClient, name: string, input: object = {}) => client.call<T>("actions_invoke", { name, input });
  const ana = runtime(anaSignup.cookie);
  const ben = runtime(benSignup.cookie);
  const visitor = runtime();
  await call(ana, "staff.name", { display: "Ana" });
  await call(ben, "staff.name", { display: "Ben" });

  // 4. One statement books a span of hours, or nothing.
  const day = utcDay(14);
  const notBooked = "Not booked. The room or the day is not on the board, the span is not 1 to 4 whole hours between 08:00 and 18:00, or someone holds one of those hours.";
  let createCalls = 0;
  let createRefused = 0;
  // A booking that inserts nothing is refused with the action's guard; book() answers 0 for it.
  const book = async (client: McpClient, input: Record<string, unknown>) => {
    createCalls++;
    try {
      return await call<{ changes: number }>(client, "bookings.create", { title: "Planning", ...input });
    } catch (error) {
      assert.equal(message(error), notBooked);
      createRefused++;
      return { changes: 0 };
    }
  };
  assert.equal((await book(ana, { room: "oak", day, start: 9, hours: 2, title: "Quarterly planning" })).changes, 2);
  assert.equal((await book(ben, { room: "oak", day, start: 10, hours: 2 })).changes, 0, "An overlapping span must be refused.");
  assert.equal((await book(ben, { room: "oak", day, start: 11, hours: 1, title: "Interview" })).changes, 1);
  const noops: Record<string, Record<string, unknown>> = {
    "a fractional hour": { room: "oak", day, start: 13.5, hours: 1 },
    "five hours": { room: "birch", day, start: 9, hours: 5 },
    "a span past 18:00": { room: "birch", day, start: 17, hours: 2 },
    "a past day": { room: "birch", day: utcDay(-2), start: 9, hours: 1 },
    "an impossible date": { room: "birch", day: "2099-02-30", start: 9, hours: 1 },
    "text that is not a date": { room: "birch", day: "tomorrow", start: 9, hours: 1 },
    "an unknown room": { room: "attic", day, start: 9, hours: 1 },
  };
  for (const [name, input] of Object.entries(noops)) assert.equal((await book(ana, input)).changes, 0, `${name} must be refused.`);
  // The Kernel's clock is UTC. West of UTC the evening is already tomorrow there, so the
  // statement accepts the UTC day before today: a person in New York books their own today at 21:00.
  assert.equal((await book(ana, { room: "birch", day: utcDay(-1), start: 9, hours: 1, title: "Late booking" })).changes, 1, "The UTC day before today is still someone's today.");
  const backstopUnique = await refusal(sql(`INSERT INTO bookings (booking_ref, room_id, day, slot, user_id, title, created_at) VALUES ('owner', 'oak', '${day}', 9, 'owner', 'Direct', 0)`));
  const backstopCheck = await refusal(sql(`INSERT INTO bookings (booking_ref, room_id, day, slot, user_id, title, created_at) VALUES ('owner', 'pine', '${day}', 9.5, 'owner', 'Direct', 0)`));
  assert.match(backstopUnique, /UNIQUE constraint failed: bookings.room_id, bookings.day, bookings.slot/);
  assert.match(backstopCheck, /CHECK constraint failed/);
  check("One-statement booking", `A two-hour booking writes two rows in one INSERT ... WITH RECURSIVE ... SELECT. An overlap and ${Object.keys(noops).join(", ")} each are refused with the guard "${notBooked}"; the statement still cannot say which rule failed. Owner SQL past the WHERE meets the partial unique index ("${backstopUnique}") and the CHECK ("${backstopCheck}").`);

  // 5. What a visitor sees, and what staff see.
  const publicRows = await call<any[]>(visitor, "availability.day", { day });
  assert.deepEqual(publicRows, [{ room: "oak", hour: 9 }, { room: "oak", hour: 10 }, { room: "oak", hour: 11 }]);
  assert.equal((await call<any[]>(visitor, "rooms.list")).length, 4);
  for (const name of ["bookings.day", "bookings.create", "bookings.recent"]) {
    assert.equal(await refusal(call(visitor, name, name === "bookings.recent" ? {} : name === "bookings.day" ? { day } : { room: "oak", day, start: 14, hours: 1, title: "Visitor" })), "authentication-required");
  }
  const routeRows = await fetch(`${url}/api/availability?day=${day}`, { headers: { host, "x-forwarded-for": freshAddress() } });
  assert.equal(routeRows.status, 200);
  assert.deepEqual((await routeRows.json()).result, publicRows);
  const benView = await call<any[]>(ben, "bookings.day", { day });
  assert.deepEqual(benView.map((row) => [row.hour, row.title, row.booked_by, row.mine]), [[9, "Quarterly planning", "Ana", 0], [10, "Quarterly planning", "Ana", 0], [11, "Interview", "Ben", 1]]);
  const [planning] = await call<any[]>(ana, "bookings.mine", { from: utcDay(0) });
  assert.deepEqual([planning.room, planning.start, planning.hours], ["oak", 9, 2]);
  assert.equal(await refusal(call(ben, "bookings.cancel", { ref: planning.ref })), "Nothing to cancel. The booking is not yours, or it is already cancelled.", "Ben cannot cancel Ana's booking.");
  assert.equal((await call(ana, "bookings.cancel", { ref: planning.ref })).changes, 2);
  assert.equal((await book(ben, { room: "oak", day, start: 9, hours: 2, title: "Retro" })).changes, 2, "Cancelled hours are free again.");
  const recent = await call<any[]>(ben, "bookings.recent");
  assert(recent.some((row) => row.ref === planning.ref && row.cancelled_at), "The cancellation stays in the activity list.");
  check("Public and staff views", "A visitor and GET /api/availability get only { room, hour }; titles and names need a session, and authenticated actions answer authentication-required. Ben sees Ana's title, is refused with the cancel guard on her booking, and books the hours she freed. The cancelled row stays for the activity list.");

  // 6. Two staff members race for overlapping hours.
  let raceWins = 0;
  const outcomes: string[] = [];
  for (let round = 0; round < 20; round++) {
    const raceDay = utcDay(30 + round);
    const settled = await Promise.allSettled([
      book(ana, { room: "cedar", day: raceDay, start: 9, hours: 2, title: "Ana's race" }),
      book(ben, { room: "cedar", day: raceDay, start: 10, hours: 2, title: "Ben's race" }),
    ]);
    const changes = settled.map((result) => result.status === "fulfilled" ? result.value.changes : `error: ${message(result.reason)}`);
    outcomes.push(changes.join("/"));
    assert.equal(changes.filter((value) => value === 2).length, 1, `Round ${round}: exactly one booking must win, got ${changes}.`);
    assert.equal(changes.filter((value) => value === 0).length, 1, `Round ${round}: the loser must be refused with the guard, got ${changes}.`);
    raceWins++;
  }
  const crowdDay = utcDay(60);
  const crowd = await Promise.allSettled(Array.from({ length: 10 }, (_, index) => book(index % 2 ? ben : ana, { room: "pine", day: crowdDay, start: 14, hours: 1, title: `Crowd ${index}` })));
  const crowdChanges = crowd.map((result) => result.status === "fulfilled" ? result.value.changes : `error: ${message(result.reason)}`);
  assert.equal(crowdChanges.filter((value) => value === 1).length, 1, `Exactly one of ten must win, got ${crowdChanges}.`);
  assert.equal(crowdChanges.filter((value) => value === 0).length, 9);
  const held = await sql(`SELECT count(*) AS n FROM bookings WHERE room_id = 'pine' AND day = '${crowdDay}' AND cancelled_at IS NULL`);
  check("Double booking under concurrency", `${raceWins} rounds of two overlapping bookings sent together: one wins with 2 rows, the other is refused with the guard (${[...new Set(outcomes)].join(", ")}, 0 standing for the guard). Ten requests for one hour: ${crowdChanges.join(",")}. Owner SQL counts ${held.rows[0].n} holder.`);

  // 7. Signals record only when a booking inserted rows, and carry who booked. Refusals count in Usage.
  const signals: any[] = [];
  let cursor: string | undefined;
  do {
    const page = await owner.call<{ signals: any[]; nextCursor?: string }>("signals_read", { deploymentId, name: "booking.created", ...(cursor ? { cursor } : {}) });
    signals.push(...page.signals);
    cursor = page.nextCursor ?? undefined;
  } while (cursor);
  const booked = Number((await sql("SELECT count(DISTINCT booking_ref) AS n FROM bookings")).rows[0].n);
  assert.equal(signals.length, booked, "booking.created records once per booking that inserted rows.");
  assert.equal(createCalls - createRefused, booked);
  const staffIds = new Set((await sql("SELECT DISTINCT user_id FROM bookings")).rows.map((row: any) => row.user_id));
  assert(signals.every((signal) => staffIds.has(signal.properties.actor)), "Each Signal names the staff member who booked.");
  const today = new Date().toISOString().slice(0, 10);
  const usage = await owner.call<{ totals: Array<{ kind: string; name: string; count: number }> }>("usage_read", { deploymentId, from: today, to: today });
  const refusedCount = usage.totals.find((row) => row.kind === "refused" && row.name === "bookings.create")?.count ?? 0;
  assert.equal(refusedCount, createRefused, "Usage counts each guard refusal under refused.");
  const cancelled = await owner.call<{ signals: any[] }>("signals_read", { deploymentId, name: "booking.cancelled" });
  assert.deepEqual(cancelled.signals.map((signal) => signal.properties.ref), [planning.ref]);
  assert(staffIds.has(cancelled.signals[0].properties.actor) && cancelled.signals[0].properties.actor !== null, "booking.cancelled names who cancelled.");
  check("Signals", `${createCalls} bookings.create calls: ${booked} bookings made and ${booked} booking.created Signals, each naming the booker as actor. One booking.cancelled names the canceller. Usage counts the ${refusedCount} guard refusals under refused.`);

  // 8. The signed check-in route for a door panel, with a key or a signature.
  const panel = await owner.call<{ key: string; id?: string; keyId?: string }>("api_keys_create", { deploymentId, name: "Door panel", actions: ["panel.checkin"] });
  const post = async (body: object, headers: Record<string, string> = {}) => {
    const response = await fetch(`${url}/api/checkin`, { method: "POST", headers: { host, "content-type": "application/json", "x-forwarded-for": freshAddress(), ...headers }, body: JSON.stringify(body) });
    return { status: response.status, body: await response.json() };
  };
  const signedBody = JSON.stringify({ room: "oak", day, hour: 9 });
  const signature = await signRoute(panelSecret, { method: "POST", path: "/api/checkin", body: signedBody });
  assert.deepEqual(await post({ room: "oak", day, hour: 9 }), { status: 401, body: { error: "invalid-signature" } });
  // A t outside the window has its own answer, so a panel with a skewed clock can tell.
  const stale = await signRoute(panelSecret, { method: "POST", path: "/api/checkin", body: signedBody }, Math.floor(Date.now() / 1000) - 400);
  assert.deepEqual(await post({ room: "oak", day, hour: 9 }, { "x-panel-signature": stale }), { status: 401, body: { error: "stale-timestamp" } });
  const signed = await fetch(`${url}/api/checkin`, { method: "POST", headers: { host, "content-type": "application/json", "x-forwarded-for": freshAddress(), "x-panel-signature": signature }, body: signedBody });
  assert.equal(signed.status, 200);
  assert.equal((await signed.json()).result.changes, 1);
  const byKey = await post({ room: "oak", day, hour: 10 }, { authorization: `Bearer ${panel.key}` });
  assert.equal(byKey.status, 200);
  assert.equal(byKey.body.result.changes, 1);
  // A second check-in for the same hour, or one for an hour nobody booked, is refused with the guard.
  const panelGuard = { status: 403, body: { error: "No booking holds this room at this hour, or it is already checked in." } };
  assert.deepEqual(await post({ room: "oak", day, hour: 10 }, { authorization: `Bearer ${panel.key}` }), panelGuard);
  assert.deepEqual(await post({ room: "oak", day, hour: 16 }, { authorization: `Bearer ${panel.key}` }), panelGuard);
  // The runtime endpoint refuses a key and says where keys work.
  const keyOnRuntimeResponse = await fetch(`${url}/_littlebox/mcp`, {
    method: "POST", headers: { host, "content-type": "application/json", accept: "application/json, text/event-stream", authorization: `Bearer ${panel.key}`, "x-forwarded-for": freshAddress() },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "actions_invoke", arguments: { name: "availability.day", input: { day } } } }),
  });
  const keyOnRuntime = { status: keyOnRuntimeResponse.status, body: await keyOnRuntimeResponse.json() };
  assert.deepEqual(keyOnRuntime, { status: 403, body: { error: "An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session." } });
  // A signed-in person from the app's own origin still needs the signature.
  const byPerson = await post({ room: "oak", day, hour: 11 }, { cookie: anaSignup.cookie!, origin: receipt.url, "sec-fetch-site": "same-origin" });
  assert.deepEqual(byPerson, { status: 401, body: { error: "invalid-signature" } });
  // The signed route is the action's only door, so the runtime endpoint refuses it for a visitor and for staff.
  const unsignedRuntime = await refusal(call(visitor, "panel.checkin", { room: "oak", day, hour: 11 }));
  assert.equal(unsignedRuntime, "Action panel.checkin answers only on its HTTP route, POST /api/checkin.");
  assert.equal(await refusal(call(ana, "panel.checkin", { room: "oak", day, hour: 11 })), unsignedRuntime);
  check("Check-in route", `POST /api/checkin refuses an unsigned body (401 invalid-signature) and a stale t (401 stale-timestamp), and accepts a signature or the panel's API key. A second check-in for the same hour and one for an hour nobody booked get 403 with the guard. A staff session cookie without a signature gets 401 invalid-signature. The runtime endpoint refuses the key with ${keyOnRuntime.status} "${keyOnRuntime.body.error}". A visitor and a staff member calling panel.checkin through actions_invoke get "${unsignedRuntime}".`);

  // 9. A booking nobody checked in within 15 minutes of its first hour is released, with
  // nobody calling: bookings.release declares "schedule": { "every": "15m" }. Ana's
  // "Late booking" for 09:00 yesterday, made in step 4, was never checked in. The owner
  // adds one the panel checked in at 10:00, in one statement so no run can release it first.
  const yesterday = utcDay(-1);
  await sql(`INSERT INTO bookings (booking_ref, room_id, day, slot, user_id, title, created_at, checked_in_at) SELECT 'pine/${yesterday}/10/0', 'pine', '${yesterday}', 10, user_id, 'Checked in', 0, 1 FROM bookings WHERE title = 'Late booking'`);
  let yesterdayHeld: any[] = [];
  for (const deadline = Date.now() + 20_000; ; await Bun.sleep(250)) {
    yesterdayHeld = await call<any[]>(visitor, "availability.day", { day: yesterday });
    if (yesterdayHeld.length === 1) break;
    assert(Date.now() < deadline, `The late booking was not released within 20 seconds: ${JSON.stringify(yesterdayHeld)}`);
  }
  assert.deepEqual(yesterdayHeld, [{ room: "pine", hour: 10 }]);
  const releasedByBen = await call<{ changes: number }>(ben, "bookings.release");
  assert.equal(releasedByBen.changes, 0, "A person's call binds an ID, and the statement releases only when it binds none.");
  const cancelledAhead = Number((await sql(`SELECT count(*) AS n FROM bookings WHERE cancelled_at IS NOT NULL AND day >= '${utcDay(0)}'`)).rows[0].n);
  assert.equal(cancelledAhead, 2, "Only Ana's cancelled planning hours are cancelled from today on.");
  const releaseUsage = await owner.call<{ totals: Array<{ kind: string; name: string; count: number }>; scheduled: Array<{ action: string; outcome: string; changes: number }> }>("usage_read", { deploymentId, from: today, to: today });
  assert(releaseUsage.scheduled.length > 0 && releaseUsage.scheduled.every((run) => run.action === "bookings.release" && run.outcome === "ok"), JSON.stringify(releaseUsage.scheduled));
  assert.equal(releaseUsage.scheduled.reduce((sum, run) => sum + run.changes, 0), 1, "Exactly one hour was released.");
  assert(!releaseUsage.totals.some((row) => row.kind === "refused" && row.name === "bookings.release"));
  const releasedSignals = await owner.call<{ signals: any[] }>("signals_read", { deploymentId, name: "booking.released" });
  assert.deepEqual(releasedSignals.signals.map((signal) => signal.properties), [{}]);
  check("Release unclaimed bookings", `bookings.release, declared every 15 minutes and run here every 2 seconds, released Ana's unclaimed 09:00 booking for ${yesterday} with nobody calling; GET availability for that day now holds only the checked-in ${JSON.stringify(yesterdayHeld[0])}, and no booking from today on was touched. Ben calling bookings.release himself answers ${JSON.stringify(releasedByBen)}. usage_read lists ${releaseUsage.scheduled.length} runs, all ok, that changed 1 row between them, and one booking.released Signal records the run that did.`);

  // 10. The Action Contract, and conform with a session token and with an API key.
  const contractDir = await copyOf("contract");
  const cli = async (args: string[]) => {
    const child = Bun.spawn(["bun", "run", path.join(repo, "scripts/littlebox.ts"), ...args], { cwd: repo, stdout: "pipe", stderr: "pipe" });
    const [code, out, err] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
    return { code, out, err };
  };
  const written = await cli(["contract", "--dir", contractDir]);
  assert.equal(written.code, 0, written.err);
  const { contract, warnings } = generateActionContract({ actionsSource: capsule["actions.json"], schemaSource: capsule["schema.sql"] });
  const unknownColumns = contract.actions.flatMap((action: any) => action.output?.columns?.filter((column: any) => column.type === "unknown").map((column: any) => `${action.name}.${column.name}`) ?? []);
  const unnamed = contract.actions.flatMap((action: any) => (action.output?.unnamed ?? []).map((column: unknown) => `${action.name}: ${JSON.stringify(column)}`));
  const conformer = await signup("conform@office.example");
  assert.equal(conformer.status, 200);
  const token = conformer.cookie!.split("=").slice(1).join("=");
  const contractFile = path.join(contractDir, "action-contract.json");
  const withSecret = await cli(["conform", "--url", receipt.url, "--contract", contractFile, "--token", token, "--secret", `panel_hook=${panelSecret}`]);
  const withKey = await cli(["conform", "--url", receipt.url, "--contract", contractFile, "--token", token, "--key", panel.key]);
  const keyOnly = await cli(["conform", "--url", receipt.url, "--contract", contractFile, "--key", panel.key]);
  const totals = (run: { out: string }) => run.out.trim().split("\n").at(-1)!;
  for (const [name, run] of Object.entries({ withSecret, withKey, keyOnly })) assert.equal(run.code, 0, `${name}:\n${run.out}${run.err}`);
  assert.match(totals(withSecret), /^\d+ passed, 0 failed, 0 skipped/);
  assert.match(totals(withKey), /^\d+ passed, 0 failed, 0 skipped/);
  // The same contract in-process from fresh addresses, so the anonymous limit never waits.
  const fromFresh: typeof fetch = (input, init) => { const headers = new Headers(init?.headers); headers.set("x-forwarded-for", freshAddress()); return fetch(input, { ...init, headers }); };
  const inProcess = await runConformance(contract, kernelTarget({ url: receipt.url, token, key: panel.key, fetch: fromFresh }));
  assert.equal(inProcess.failed, 0, formatConformanceReport(inProcess));
  check("Action Contract", `contract writes ${contract.actions.length} actions; warnings ${JSON.stringify(warnings ?? [])}; unnamed ${JSON.stringify(unnamed)}; columns typed unknown ${unknownColumns.join(", ")}. conform --token --secret: "${totals(withSecret)}". conform --token --key: "${totals(withKey)}". conform --key alone: "${totals(keyOnly)}".`);

  // 11. Chrome under the Content Security Policy: visitor and staff, light and dark.
  if (process.env.LITTLEBOX_SHOWCASE_BROWSER !== "0") {
    if (shots) await mkdir(shots, { recursive: true });
    const browser = await chromium.launch({ channel: "chrome" });
    try {
      const appUrl = new URL(receipt.url);
      const staffToken = (await signupSession("dana@office.example")).split("=").slice(1).join("=");
      for (const [who, scheme] of [["visitor", "light"], ["staff", "light"], ["staff", "dark"]] as const) {
        const context = await browser.newContext({ colorScheme: scheme, viewport: { width: 1180, height: 900 } });
        if (who === "staff") await context.addCookies([{ name: "littlebox_app_session", value: staffToken, domain: appUrl.hostname, path: "/" }]);
        const page = await context.newPage();
        const violations: string[] = [];
        page.on("console", (entry) => { if (/Content Security Policy|Refused to/i.test(entry.text())) violations.push(entry.text()); });
        page.on("pageerror", (error) => violations.push(`page error: ${error.message}`));
        await page.addInitScript(() => document.addEventListener("securitypolicyviolation", (event) => console.error(`Refused to load ${event.blockedURI} by ${event.violatedDirective}`)));
        const response = await page.goto(`${receipt.url}/`, { waitUntil: "load" });
        assert.equal(response?.status(), 200);
        const csp = response?.headers()["content-security-policy"] ?? "";
        assert.match(csp, /connect-src 'self'/);
        await page.waitForSelector("body[data-ready=true]", { timeout: 15_000 });
        await page.locator("#day").fill(day);
        await page.locator(`#grid[data-day="${day}"]`).waitFor();
        assert.equal(await page.locator("#grid tbody tr").count(), 4);
        if (who === "visitor") {
          assert.equal(await page.locator("#grid .cell.held").count(), 3);
          assert.equal(await page.getByText("Quarterly planning").count(), 0);
          assert.equal(await page.locator("button.cell").count(), 0);
        } else if (scheme === "light") {
          await page.locator('button.cell[data-room="birch"][data-hour="14"]').click();
          await page.locator("#title").fill("Design review");
          await page.locator("#hours").selectOption("2");
          await page.locator("#confirm").click();
          await page.locator("#status.ok").waitFor();
          assert.equal(await page.locator("#status").textContent(), "Booked 2 hours.");
          await page.locator('button.cell[data-room="birch"][data-hour="15"]').waitFor({ state: "detached" });
          assert.equal(await page.locator(".cell.mine").count(), 2);
          // With the network gone, moving to the next day says so on the page instead of failing silently.
          await context.setOffline(true);
          await page.locator("#next").click();
          await page.locator("#status.error").filter({ hasText: "could not reach its server" }).waitFor({ timeout: 10_000 });
          await context.setOffline(false);
        } else {
          await page.locator(".cell.mine").first().waitFor();
          await page.locator("#mine li").first().waitFor();
        }
        assert.deepEqual(violations, [], `${who} ${scheme} broke under the Content Security Policy.`);
        if (shots) await page.screenshot({ path: path.join(shots, `booking-${who}-${scheme}.png`), fullPage: true });
        await context.close();
      }
    } finally { await browser.close(); }
    check("Chrome", `The visitor and staff pages load under "${"connect-src 'self'"}" with no policy violation or page error; staff book two hours from the grid, and an offline day change shows a sentence instead of an unhandled error.${shots ? ` Screenshots in ${shots}.` : ""}`);
  }

  console.log(JSON.stringify({ app: "booking", checks: findings.length, findings }, null, 2));

  async function signupSession(email: string) {
    const result = await signup(email);
    assert.equal(result.status, 200, JSON.stringify(result.body));
    return result.cookie!;
  }
} finally {
  kernel?.kill();
  if (kernel) await kernel.exited;
  await rm(scratch, { recursive: true, force: true });
}

async function copyOf(name: string) {
  const root = path.join(scratch, name);
  await cp(source, root, { recursive: true, filter: (file) => !/(replay\.ts|README\.md|\.littlebox)$/.test(file) });
  return root;
}

async function publish(projectId: string, slug: string, files: Record<string, string>, initialSignupPolicy?: { mode: string; allowlist: string[] }) {
  const root = await copyOf(`publish-${projectId}-${crypto.randomUUID()}`);
  for (const [name, text] of Object.entries(files)) await Bun.write(path.join(root, name), text);
  const compiled = await buildLocal(root);
  const project = { id: projectId, name: projectId, slug, manifest: { version: 1, visibility: "public" }, files: compiled.files };
  const owner = new McpClient(`${url}/mcp`, { fetch: ownerFetch });
  const upload = await owner.upload(new TextEncoder().encode(JSON.stringify({ project, visibility: "public", artifact: compiled.artifact, ...(initialSignupPolicy ? { initialSignupPolicy } : {}) })), projectId);
  try {
    const plan = await owner.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
    return await owner.call<{ id: string; hostname: string; url: string }>("release_apply", { planId: plan.planId });
  } finally { await owner.cancel(upload.transferId); }
}

/** `littlebox publish` from a copy of this folder, with a token that holds read, write and publish and not manage_users. */
async function publishWithCli(projectId: string, slug: string, signupPolicy?: { mode: string; allowlist: string[] }) {
  const root = await copyOf(`cli-${projectId}-${crypto.randomUUID()}`);
  const owner = new McpClient(`${url}/mcp`, { fetch: ownerFetch });
  const { token } = await owner.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish"], ttlSeconds: 600 });
  const project = { id: projectId, name: projectId, slug, manifest: { version: 1, visibility: "public", ...(signupPolicy ? { signupPolicy } : {}) } };
  return await publishLocal(root, { url, project }, token) as { id: string; hostname: string; url: string };
}

function ownerFetch(input: RequestInfo | URL, init?: RequestInit) {
  const headers = new Headers(init?.headers); headers.set("cookie", ownerCookie);
  return fetch(input, { ...init, headers });
}
