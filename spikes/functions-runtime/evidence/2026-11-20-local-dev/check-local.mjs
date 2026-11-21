import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { localKernel, readSettings } from "../../../../scripts/konae.ts";

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.join(here, "app");
const settings = await readSettings(appRoot);
const kernel = await localKernel(appRoot, settings, { build: false, port: "8796" });
const origin = "http://127.0.0.1:8796";
const host = `${settings.slug}.localhost`;
const headers = (extra = {}) => ({ Host: host, ...extra });
const results = { checks: {}, observations: {}, startedAt: new Date().toISOString() };

async function request(pathname, options = {}) {
  const started = performance.now();
  for (let attempt = 0; attempt < 80; attempt++) {
    const args = ["curl", "-sS", "--max-time", "10", "-i", `${origin}${pathname}`, "-H", `Host: ${host}`];
    for (const [name, value] of Object.entries(options.headers ?? {})) args.push("-H", `${name}: ${value}`);
    if (options.method) args.push("-X", options.method);
    if (options.body !== undefined) args.push("--data-binary", options.body);
    const child = Bun.spawn(args, { stdout: "pipe", stderr: "pipe" });
    const [code, output, error] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
    if (code !== 0) {
      if (![7, 52, 56].includes(code)) assert.equal(code, 0, error || "curl failed");
      await new Promise((resolve) => setTimeout(resolve, 250));
      continue;
    }
    const boundary = output.indexOf("\r\n\r\n");
    const head = output.slice(0, boundary);
    const text = output.slice(boundary + 4);
    const status = Number(/^HTTP\/\S+ (\d+)/.exec(head)?.[1]);
    const cookie = /^set-cookie:\s*([^\r\n]+)/im.exec(head)?.[1];
    const response = { status, ok: status >= 200 && status < 300, headers: { get: (name) => name.toLowerCase() === "set-cookie" ? cookie : null } };
    let body;
    try { body = text ? JSON.parse(text) : null; } catch { body = text; }
    return { response, body, elapsedMs: Number((performance.now() - started).toFixed(3)) };
  }
  throw new Error(`Local Kernel did not answer ${pathname} after transient connection failures.`);
}

async function createOrLogin(email, password) {
  let answer = await request("/_konae/auth/signup", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email, password }),
  });
  if (!answer.response.ok) {
    answer = await request("/_konae/auth/login", {
      method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email, password }),
    });
  }
  assert.equal(answer.response.status, 200, `Local signup or login failed with ${answer.response.status}`);
  const cookie = answer.response.headers.get("set-cookie")?.split(";", 1)[0];
  assert.ok(cookie, "Local signup or login returned no app session cookie");
  return cookie;
}

async function getNotes(cookie) {
  return request("/api/notes/", { headers: cookie ? { Cookie: cookie } : {} });
}

async function waitForVersion(cookie, wanted) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    try {
      const answer = await getNotes(cookie);
      if (answer.response.ok && answer.body.version === wanted) return answer;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Local dev did not reload function bundle ${wanted} within 20 seconds.`);
}

async function waitForServer() {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    try { if ((await getNotes()).response.status === 401) return; } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("konae dev did not return the anonymous authentication refusal within 20 seconds.");
}

const sourcePath = path.join(appRoot, "app/functions/notes.ts");
let source = await readFile(sourcePath, "utf8");
const initialSource = source.replace(/const BUILD_VERSION = "v[12]";/, 'const BUILD_VERSION = "v1";');
if (initialSource !== source) await writeFile(sourcePath, initialSource);
await waitForServer();

const stamp = `${Date.now()}`;
const password = "konae-local-demo";
const actorA = await createOrLogin("ada@example.test", password);
const actorB = await createOrLogin("bea@example.test", password);
await waitForVersion(actorA, "v1");

const anonymous = await getNotes();
assert.equal(anonymous.response.status, 401);
results.checks.anonymous = { pass: true, status: anonymous.response.status };

const emptyA = await getNotes(actorA);
assert.equal(emptyA.response.status, 200);
assert.equal(emptyA.body.notes.length, 0);

const invalid = await request("/api/notes/", {
  method: "POST", headers: { Cookie: actorA, "Content-Type": "application/json", "Idempotency-Key": `invalid-${stamp}` },
  body: JSON.stringify({ title: "" }),
});
assert.equal(invalid.response.status, 400);
assert.equal((await getNotes(actorA)).body.notes.length, 0);
results.checks.validation = { pass: true, status: invalid.response.status, rowsAfter: 0 };

const createKey = `create-${stamp}`;
const created = await request("/api/notes/", {
  method: "POST", headers: { Cookie: actorA, "Content-Type": "application/json", "Idempotency-Key": createKey },
  body: JSON.stringify({ title: "synthetic local note" }),
});
assert.equal(created.response.status, 201);
const replay = await request("/api/notes/", {
  method: "POST", headers: { Cookie: actorA, "Content-Type": "application/json", "Idempotency-Key": createKey },
  body: JSON.stringify({ title: "synthetic local note" }),
});
assert.equal(replay.response.status, created.response.status);
assert.deepEqual(replay.body, created.body);
const conflict = await request("/api/notes/", {
  method: "POST", headers: { Cookie: actorA, "Content-Type": "application/json", "Idempotency-Key": createKey },
  body: JSON.stringify({ title: "different payload" }),
});
assert.equal(conflict.response.status, 409);
results.checks.idempotency = { pass: true, createStatus: created.response.status, replayStatus: replay.response.status, conflictStatus: conflict.response.status };

const ownRows = await getNotes(actorA);
const otherRows = await getNotes(actorB);
assert.equal(ownRows.body.notes.length, 1);
assert.equal(otherRows.body.notes.length, 0);
results.checks.rowAuthorization = { pass: true, ownerRows: ownRows.body.notes.length, otherUserRows: otherRows.body.notes.length };

const batchKey = `batch-${stamp}`;
const batch = await request("/api/notes/batch", {
  method: "POST", headers: { Cookie: actorA, "Content-Type": "application/json", "Idempotency-Key": batchKey },
  body: JSON.stringify({ titles: ["synthetic batch one", "synthetic batch two"] }),
});
assert.equal(batch.response.status, 201);
assert.equal((await getNotes(actorA)).body.notes.length, 3);
const guardedBatch = await request("/api/notes/batch-guard", {
  method: "POST", headers: { Cookie: actorA, "Content-Type": "application/json", "Idempotency-Key": `guard-${stamp}` },
  body: "{}",
});
assert.equal(guardedBatch.response.status, 403);
assert.equal((await getNotes(actorA)).body.notes.length, 3);
assert.equal((await getNotes(actorB)).body.notes.length, 0);
results.checks.jsonEachBatch = { pass: true, status: batch.response.status, ownerRows: 3, otherUserRows: 0, guardStatus: guardedBatch.response.status, guardResponse: guardedBatch.body, rowsAfterGuard: 3 };

const forbidden = await request("/api/notes/forbidden", { headers: { Cookie: actorB } });
assert.ok(forbidden.response.status >= 400, "An undeclared action unexpectedly succeeded");
assert.equal((await getNotes(actorA)).body.notes.length, 3);
results.checks.actionAllowlist = { pass: true, status: forbidden.response.status, response: forbidden.body, rowsAfter: 3 };

source = await readFile(sourcePath, "utf8");
await writeFile(sourcePath, source.replace('const BUILD_VERSION = "v1";', 'const BUILD_VERSION = "v2";'));
const reloaded = await waitForVersion(actorA, "v2");
results.checks.versionReload = { pass: true, version: reloaded.body.version };

const workerPids = () => Bun.spawnSync(["ps", "-axo", "pid=,command="]).stdout.toString()
  .split("\n").filter((line) => line.includes("/tmp/konae-deno-core-target/release/konae-functions-spike"))
  .map((line) => line.trim().split(/\s+/, 1)[0]).filter(Boolean).sort();
const manifestPath = path.join(appRoot, "app/functions.json");
const manifestSource = await readFile(manifestPath, "utf8");
let emptyManifest = false;
let functionsDisabled = false;
try {
  let refusal;
  try { await kernel.run(["module", "functions", "off"]); } catch (error) { refusal = error.message; }
  assert.match(refusal ?? "", /in use by the published app/);
  results.checks.moduleOffWhileDeclared = { pass: true, refused: true, message: refusal };
  await writeFile(manifestPath, JSON.stringify({ version: 1, functions: {} }, null, 2) + "\n");
  emptyManifest = true;
  const emptyDeadline = Date.now() + 20_000;
  let emptyRoute;
  while (Date.now() < emptyDeadline) {
    try { emptyRoute = await getNotes(actorA); if (emptyRoute.response.status === 404) break; } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  assert.equal(emptyRoute?.response.status, 404, "Removing the declaration did not reload the local app");
  await kernel.run(["module", "functions", "off"]);
  functionsDisabled = true;
  const moduleState = JSON.parse(await kernel.run(["modules"]));
  assert.equal(moduleState.modules.functions.state.on, false);
  const workersBefore = workerPids();
  const disabled = await getNotes(actorA);
  assert.equal(disabled.response.status, 404);
  const workersAfter = workerPids();
  assert.deepEqual(workersAfter, workersBefore, "Functions-off started or replaced a runner process");
  results.checks.moduleOff = { pass: true, status: disabled.response.status, workersBefore: workersBefore.length, workersAfter: workersAfter.length };
} finally {
  if (functionsDisabled) await kernel.run(["module", "functions", "on"]);
  if (emptyManifest) {
    await writeFile(manifestPath, manifestSource);
    await waitForVersion(actorA, "v2");
  }
}

const today = new Date().toISOString().slice(0, 10);
const tomorrowDate = new Date(Date.now() + 86_400_000).toISOString().slice(0, 10);
const usage = JSON.parse(await kernel.run(["events", "--from", today, "--to", tomorrowDate]));
const series = usage.summary.functionLatency.series;
assert.ok(series.some((item) => item.name === "notes" && item.samples >= 1), "Kernel Usage has no function latency sample");
results.observations.usage = series.filter((item) => item.name === "notes").map(({ version, samples, replays, failures, total, execution }) => ({ version, samples, replays, failures, total, execution }));
results.observations.timings = { anonymousMs: anonymous.elapsedMs, invalidMs: invalid.elapsedMs, createMs: created.elapsedMs, replayMs: replay.elapsedMs, conflictMs: conflict.elapsedMs };
results.finishedAt = new Date().toISOString();
await writeFile(path.join(here, "results.json"), `${JSON.stringify(results, null, 2)}\n`);
console.log(JSON.stringify(results, null, 2));
