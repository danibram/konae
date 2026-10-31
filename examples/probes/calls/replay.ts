// Probe: one app calls another (ADR-0019). The tasks app escalates a task to
// the desk app's signed route with a call action; the Kernel sends it from its
// own process, through the desk's route code, and the tasks app's then
// mutation stores what the desk answered, including the desk's guard refusal.
// It also checks a call that sends an API key, a call signed with the wrong
// secret, the refusals at both doors, the per-app limit, the outbox, Usage,
// and connects with a public GET route read from another app's origin.
//
//   KONAE_PROBE_PORT=23070 bun run examples/probes/calls/replay.ts
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { probe } from "../_kit";

const here = import.meta.dir;
const deskSecret = `desk-${crypto.randomUUID()}`;
const reused = "Another of your projects already uses this secret value. A signature does not name the app, so a request signed for one would be accepted by the other. Set a different value.";

type Task = { id: number; title: string; escalation_status: number | null; escalation_note: string | null };
type Call = { callId: string; action: string; state: string; attempts: number; status: number | null; error: string | null; then: { action: string; state: string; message: string | null } | null };

await probe("calls", 23070, async (kit) => {
  const { url, owner } = kit;
  const port = new URL(url).port;
  const desk = await kit.publish(path.join(here, "desk"), { projectId: "probe_desk", visibility: "public", secrets: { tickets_hook: deskSecret } });
  const deskOrigin = `http://${desk.host}:${port}`;

  // The tasks app holds the desk's secret to sign with. A route secret may not
  // repeat another project's value; a secret set for calls only may.
  await owner.call("project_create", { projectId: "probe_tasks", name: "probe_tasks", slug: "probe-tasks", manifest: { version: 1, visibility: "public" } });
  const refusedReuse = await kit.errorText(owner.call("secret_set", { projectId: "probe_tasks", name: "desk_tickets", value: deskSecret }));
  assert.equal(refusedReuse, reused);
  await owner.call("secret_set", { projectId: "probe_tasks", name: "desk_tickets", value: deskSecret, use: "call" });
  await owner.call("secret_set", { projectId: "probe_tasks", name: "desk_guess", value: `guess-${crypto.randomUUID()}`, use: "call" });
  const deskKey = await owner.call<{ key: string; keyId: string }>("api_keys_create", { deploymentId: desk.deploymentId, name: "Tasks app", actions: ["tickets.count"] });
  await owner.call("secret_set", { projectId: "probe_tasks", name: "desk_key", value: deskKey.key, use: "call" });
  const secrets = await owner.call<{ secrets: { name: string; use: string }[] }>("secrets_list", { projectId: "probe_tasks" });
  assert.deepEqual(secrets.secrets.map((secret) => [secret.name, secret.use]), [["desk_guess", "call"], ["desk_key", "call"], ["desk_tickets", "call"]]);
  kit.pass("A secret for calls only", `secret_set refuses the desk's value as a route secret of the tasks app ("${refusedReuse.slice(0, 54)}...") and takes it with "use": "call"; secrets_list names all three with use call, and never a value.`);

  const tasksSource = (await readFile(path.join(here, "tasks/actions.json"), "utf8")).replaceAll("http://desk.localhost", deskOrigin).replaceAll("http://nobody.localhost", `http://nobody.localhost:${port}`);
  const tasks = await kit.publish(path.join(here, "tasks"), { projectId: "probe_tasks", visibility: "public", files: { "actions.json": tasksSource }, manifest: { connects: [deskOrigin] } });
  const cookie = await kit.signup(tasks.host, "ana@calls.test");
  const ana = kit.runtime(tasks.host, cookie);
  const visitor = kit.runtime(tasks.host);
  await ana.call("actions_invoke", { name: "tasks.add", input: { title: "Printer is down" } });
  await ana.call("actions_invoke", { name: "tasks.add", input: { title: "Badge reader" } });
  const mine = async () => (await ana.call<Task[]>("actions_invoke", { name: "tasks.mine" }));
  const until = async <T>(read: () => Promise<T>, done: (value: T) => boolean, what: string) => {
    for (const deadline = Date.now() + 15_000; Date.now() < deadline; await Bun.sleep(100)) {
      const value = await read();
      if (done(value)) return value;
    }
    assert.fail(`Timed out waiting for ${what}.`);
  };

  // A call to the desk's signed route, sent by the Kernel through the desk's route code.
  const queued = await ana.call<{ callId: string; queued: boolean }>("actions_invoke", { name: "tasks.escalate", input: { taskId: 1, title: "Printer is down" } });
  assert.equal(queued.queued, true);
  assert.match(queued.callId, /^call_[0-9a-f]{32}$/);
  const first = await until(mine, (rows) => rows[0].escalation_status !== null, "the first escalation");
  assert.deepEqual([first[0].escalation_status, first[0].escalation_note], [200, null]);
  const board = await kit.appFetch(desk.host)(`${url}/api/board`);
  assert.deepEqual(await board.json(), { result: [{ ref: 1, title: "Printer is down" }] });
  kit.pass("A signed call to another app", `actions_invoke answers { callId, queued: true }. The Kernel signs POST ${deskOrigin}/hooks/tickets with desk_tickets in X-Desk-Signature, the desk's route verifies it and inserts the ticket, and the tasks app's then mutation stores status ${first[0].escalation_status}.`);

  // The same task again: the desk's guard refuses, and its words reach the tasks app.
  await ana.call("actions_invoke", { name: "tasks.escalate", input: { taskId: 1, title: "Printer is down" } });
  const second = await until(mine, (rows) => rows[0].escalation_status === 403, "the desk's guard refusal");
  assert.equal(second[0].escalation_note, "A ticket for this task is already open.");
  kit.pass("The callee's guard reaches the caller's then", `A second escalation of task 1 is refused by the desk's guard with 403; tasks.escalated stores status ${second[0].escalation_status} and "${second[0].escalation_note}" from /body/error.`);

  // The same route with the wrong secret: the desk's signature check refuses it.
  await ana.call("actions_invoke", { name: "tasks.forge", input: { taskId: 2, title: "Badge reader" } });
  const forged = await until(mine, (rows) => rows[1].escalation_status !== null, "the forged call");
  assert.deepEqual([forged[1].escalation_status, forged[1].escalation_note], [401, "invalid-signature"]);
  kit.pass("The callee's callers check", `A call signed with another secret gets ${forged[1].escalation_status} ${forged[1].escalation_note} from the desk's route, as a request from outside would, and the then mutation stores it.`);

  // A call that sends an API key to the desk's keyed GET route.
  await ana.call("actions_invoke", { name: "desk.count", input: {} });
  const counts = await until(() => ana.call<{ open: number; status: number }[]>("actions_invoke", { name: "desk.counts" }), (rows) => rows.length > 0, "the keyed count");
  assert.deepEqual(counts, [{ open: 1, status: 200 }]);
  kit.pass("A call that sends an API key", `desk.count sends GET /api/count with the key desk_key holds as Authorization: Bearer; the route takes keys only, answers { result: [{ open: 1 }] }, and desk.counted stores open ${counts[0].open} from /body/result/0/open.`);

  const notices = async () => (await readFile(path.join(kit.dataDir, "mailbox.jsonl"), "utf8").catch(() => ""))
    .trim().split("\n").filter(Boolean).map((line) => JSON.parse(line))
    .filter((mail) => mail.purpose === "notify" && mail.action === "tasks.escalated");
  const mail = await until(notices, (messages) => messages.length >= 3, "the then mutation's notices");
  assert.equal(mail.length, 3);
  assert(mail.every((message) => message.to === "owner@probe.example"));
  assert.deepEqual(mail.map((message) => message.subject).sort(), ["Task 1 answered 200", "Task 1 answered 403", "Task 2 answered 401"]);
  const noticeUsage = await owner.call<{ totals: { kind: string; name: string; count: number }[] }>("usage_read", {
    deploymentId: tasks.deploymentId, from: new Date().toISOString().slice(0, 10), to: new Date().toISOString().slice(0, 10),
  });
  assert.equal(noticeUsage.totals.find((row) => row.kind === "notify" && row.name === "tasks.escalated: queued")?.count, 3);
  assert.equal(noticeUsage.totals.find((row) => row.kind === "notify" && row.name === "desk.counted: no-account")?.count, 1);
  kit.pass("The then mutation's notices", "Three changed then mutations each emailed the owner with their bound inputs. A notice to actor records no-account because a call's answer has no caller.");

  // Only the Kernel runs a then mutation.
  const thenOnly = await kit.errorText(ana.call("actions_invoke", { name: "tasks.escalated", input: { id: 2, status: 200, note: null } }));
  assert.equal(thenOnly, "Action tasks.escalated runs only after the call tasks.escalate answers.");
  const signedOut = await kit.errorText(visitor.call("actions_invoke", { name: "tasks.escalate", input: { taskId: 2, title: "x" } }));
  assert.equal(signedOut, "authentication-required");
  const badInput = await kit.errorText(ana.call("actions_invoke", { name: "tasks.escalate", input: { taskId: "2", title: "x" } }));
  assert.equal(badInput, "taskId must be a number.");
  kit.pass("Who may start a call and write its answer", `A person writing tasks.escalated answers "${thenOnly}"; a visitor starting the authenticated call answers ${signedOut}; a text taskId answers "${badInput}", and none is queued.`);

  // The outbox and Usage.
  const outbox = await owner.call<{ calls: Call[] }>("calls_read", { deploymentId: tasks.deploymentId });
  assert.deepEqual(outbox.calls.map((call) => [call.action, call.state, call.status, call.then?.state, call.then?.message ?? null]), [
    ["desk.count", "delivered", 200, "done", null],
    ["tasks.forge", "failed", 401, "done", null],
    ["tasks.escalate", "failed", 403, "done", null],
    ["tasks.escalate", "delivered", 200, "done", null],
  ]);
  assert(outbox.calls.every((call) => call.attempts === 1));
  const today = new Date().toISOString().slice(0, 10);
  const usage = await owner.call<{ totals: { kind: string; name: string; count: number }[] }>("usage_read", { deploymentId: tasks.deploymentId, from: today, to: today });
  const total = (kind: string, name: string) => usage.totals.find((row) => row.kind === kind && row.name === name)?.count ?? 0;
  assert.deepEqual([total("call", "tasks.escalate"), total("call", "tasks.forge"), total("call", "desk.count"), total("action", "tasks.escalated")], [2, 1, 1, 3]);
  const deskUsage = await owner.call<{ totals: { kind: string; name: string; count: number }[] }>("usage_read", { deploymentId: desk.deploymentId, from: today, to: today });
  const deskTotal = (kind: string, name: string) => deskUsage.totals.find((row) => row.kind === kind && row.name === name)?.count ?? 0;
  assert.deepEqual([deskTotal("route", "POST /hooks/tickets"), deskTotal("refused", "tickets.open"), deskTotal("route", "GET /api/count")], [1, 1, 1]);
  kit.pass("Outbox and Usage", `calls_read lists the four calls newest first with state, status and what the then did, each sent once. The tasks app counts call ${total("call", "tasks.escalate")} for tasks.escalate and action ${total("action", "tasks.escalated")} for its then; the desk counts its route, its guard refusal and its keyed GET, as for a request from outside.`);

  // A failed call is final at once when no app answers on the installation's host.
  await ana.call("actions_invoke", { name: "nobody.ping", input: {} });
  const nobody = await until(() => owner.call<{ calls: Call[] }>("calls_read", { deploymentId: tasks.deploymentId }), (page) => page.calls[0].state !== "pending", "the call to no app");
  assert.deepEqual([nobody.calls[0].state, nobody.calls[0].error, nobody.calls[0].attempts], ["failed", "app-not-found", 1]);
  // The per-app limit: 20 queued calls in a minute, then a refusal.
  let queuedCount = 5;
  let limited = "";
  while (!limited) {
    try {
      await ana.call("actions_invoke", { name: "nobody.ping", input: {} });
      queuedCount += 1;
    } catch (error) {
      limited = (error as Error).message;
    }
    assert(queuedCount <= 20, "The Kernel queued more than 20 calls in a minute.");
  }
  assert.equal(queuedCount, 20);
  assert.equal(limited, "This app reached its limit of calls in the last minute. Try again in a minute.");
  kit.pass("Limits", `A call to a host under the installation's suffix that no app holds fails at once with app-not-found and is not retried. After ${queuedCount} calls in a minute the next answers "${limited}"`);

  // Refusals at both doors, and the Kernel's own.
  const tasksFiles = { "schema.sql": await readFile(path.join(here, "tasks/schema.sql"), "utf8") };
  const program = JSON.parse(tasksSource);
  const withUrl = (target: string) => JSON.stringify({ ...program, actions: { ...program.actions, "tasks.escalate": { ...program.actions["tasks.escalate"], url: target } } });
  const metadata = withUrl("http://169.254.169.254/latest/meta-data");
  const metadataGate = await kit.gateRefusal({ ...tasksFiles, "actions.json": metadata });
  const metadataKernel = await kit.planRefusal(path.join(here, "tasks"), "probe_tasks", { "actions.json": metadata });
  assert.equal(metadataGate.detail, "Action tasks.escalate calls the IP address 169.254.169.254. Name a host instead.");
  assert.equal(metadataKernel, metadataGate.detail);
  const admin = withUrl(`${deskOrigin}/_konae/mcp`);
  const adminKernel = await kit.planRefusal(path.join(here, "tasks"), "probe_tasks", { "actions.json": admin });
  assert.equal(adminKernel, "Action tasks.escalate calls a path under /_konae, /oauth, /.well-known or /mcp, which belong to the Kernel.");
  assert.equal((await kit.gateRefusal({ ...tasksFiles, "actions.json": admin })).detail, adminKernel);
  const plain = await kit.planRefusal(path.join(here, "tasks"), "probe_tasks", { "actions.json": withUrl("http://api.example.com/hooks") });
  assert.equal(plain, "Action tasks.escalate calls http://api.example.com, which is not an app of this installation. Use https.");
  const unset = JSON.stringify({ ...program, actions: { ...program.actions, "tasks.escalate": { ...program.actions["tasks.escalate"], auth: { secret: "desk_other" } } } });
  const unsetKernel = await kit.planRefusal(path.join(here, "tasks"), "probe_tasks", { "actions.json": unset });
  assert.equal(unsetKernel, "Action tasks.escalate calls with the secret desk_other, which is not set for this project. Set it with secret_set before publishing.");
  kit.pass("Refusals at publish", `Both doors refuse the metadata address ("${metadataKernel}") and the Kernel's admin path; the Kernel alone, which knows the installation and the secrets, refuses http to an outside host ("${plain}") and an unset secret.`);

  // connects: the tasks page may read the desk's public GET route, and nothing else changes.
  const page = await kit.appFetch(tasks.host)(`${url}/`);
  const policy = page.headers.get("content-security-policy") ?? "";
  assert(policy.includes(`connect-src 'self' ${deskOrigin};`), policy);
  const fromTasks = await kit.appFetch(desk.host)(`${url}/api/board`, { headers: { origin: `http://${tasks.host}:${port}` } });
  assert.equal(fromTasks.headers.get("access-control-allow-origin"), "*");
  const keyed = await kit.appFetch(desk.host)(`${url}/api/count`, { headers: { origin: `http://${tasks.host}:${port}`, authorization: `Bearer ${deskKey.key}` } });
  assert.equal(keyed.headers.get("access-control-allow-origin"), null);
  kit.pass("connects", `The tasks page's policy reads connect-src 'self' ${deskOrigin}. The desk's open GET /api/board answers Access-Control-Allow-Origin: *, so that page reads it with fetch; the keyed GET /api/count sends no CORS header, so a key stays with programs and calls.`);
});
