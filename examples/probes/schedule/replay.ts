// Probe: scheduled work (ADR-0017). Decides the matrix cell for work that has
// to happen when nobody asks. A reminder app surfaces due notes from a
// "schedule" on a mutation; the Kernel's timer runs it with no caller.
//
// The probe starts its Kernel with LITTLEBOX_SCHEDULE_MIN_SECONDS=2, so a
// schedule declared "every": "5m" runs every 2 seconds and the probe waits
// for runs instead of minutes.
//
//   LITTLEBOX_PROBE_PORT=23060 bun run examples/probes/schedule/replay.ts
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { probe } from "../_kit";

process.env.LITTLEBOX_SCHEDULE_MIN_SECONDS = "2";
const here = import.meta.dir;
type Run = { action: string; at: number; outcome: "ok" | "failed"; changes: number; error?: string };
type Usage = { totals: { kind: string; name: string; count: number }[]; scheduled: Run[] };
const today = () => new Date().toISOString().slice(0, 10);

await probe("schedule", 23060, async (kit) => {
  const { owner } = kit;

  // Both doors: the declaration the app uses passes, and each refusal has the same words.
  const schema = await readFile(path.join(here, "schema.sql"), "utf8");
  const program = await readFile(path.join(here, "actions.json"), "utf8");
  const variants: Record<string, [(actions: any) => void, string]> = {
    "on a query": [(a) => { a["notes.inbox"].schedule = { every: "15m" }; }, "Action notes.inbox is a query, so it cannot declare a schedule. Declare it on a mutation."],
    "on a mutation with input": [(a) => { a["notes.add"].schedule = { daily: "08:00" }; }, "Action notes.add declares a schedule, so it cannot take input. A scheduled run has no caller to supply it; bind only actor, kernel and literal values."],
    "every 4 minutes": [(a) => { a["notes.surface"].schedule = { every: "4m" }; }, 'Action notes.surface runs every 4m, more often than every 5 minutes. Use "5m" or longer.'],
    "every 25 hours": [(a) => { a["notes.surface"].schedule = { every: "25h" }; }, 'Action notes.surface runs every 25h, less often than once a day. Use "daily" or a period of 24 hours or less.'],
    "daily at 24:00": [(a) => { a["notes.surface"].schedule = { daily: "24:00" }; }, "Action notes.surface has an invalid daily time. Use HH:MM in UTC, from 00:00 to 23:59."],
    "a cron expression": [(a) => { a["notes.surface"].schedule = "0 8 * * *"; }, 'Action notes.surface has an invalid schedule. Declare "every" with minutes or hours, such as "15m" or "6h", or "daily" with a UTC time, such as "02:00".'],
    "a misspelled schedule": [(a) => { a["notes.surface"].shedule = a["notes.surface"].schedule; delete a["notes.surface"].schedule; }, "Action notes.surface has a key shedule that a mutation action does not take."],
    "subscribe on a query": [(a) => { a["notes.inbox"].subscribe = true; }, "Action notes.inbox declares subscribe, but the Kernel pushes no updates to a page. Poll a query that reads rows after a cursor instead."],
  };
  const answers: string[] = [];
  for (const [label, [change, message]] of Object.entries(variants)) {
    const variant = JSON.parse(program);
    change(variant.actions);
    const files = { "schema.sql": schema, "actions.json": JSON.stringify(variant) };
    const gate = await kit.gateRefusal(files);
    const kernel = await kit.planRefusal(here, `probe_schedule_${label.replace(/\W+/g, "_")}`, files);
    assert.equal(gate.detail ?? gate.message, message, `${label} at the Gate`);
    assert.equal(kernel, message, `${label} at the Kernel`);
    answers.push(`${label}: "${message}"`);
  }
  const app = await kit.publish(here, { projectId: "probe_schedule", visibility: "private", signup: { mode: "open" } });
  kit.pass("Declare a schedule", `"every": "5m" on notes.surface, which binds only kernel.now, publishes. Both doors refuse, with the same words, ${answers.join("; ")}.`);

  const usage = () => owner.call<Usage>("usage_read", { deploymentId: app.deploymentId, from: today(), to: today() });
  const waitFor = async (what: string, test: (usage: Usage) => boolean, seconds = 20) => {
    const deadline = Date.now() + seconds * 1000;
    for (;;) {
      const current = await usage();
      if (test(current)) return current;
      assert(Date.now() < deadline, `No ${what} within ${seconds} seconds: ${JSON.stringify(current.scheduled.slice(0, 5))}`);
      await Bun.sleep(250);
    }
  };

  // A run with nobody asking: a due note surfaces, a future one does not.
  const cookie = await kit.signup(app.host, "rosa@probe.example");
  const person = kit.runtime(app.host, cookie);
  await person.call("actions_invoke", { name: "notes.add", input: { body: "Water the plants", remind_at: Date.now() - 1000 } });
  await person.call("actions_invoke", { name: "notes.add", input: { body: "Renew the passport", remind_at: Date.now() + 3_600_000 } });
  const first = await waitFor("run that surfaced the due note", (current) => current.scheduled.some((run) => run.changes === 1));
  const inbox = await person.call<{ body: string }[]>("actions_invoke", { name: "notes.inbox", input: {} });
  assert.deepEqual(inbox.map((note) => note.body), ["Water the plants"]);
  const ran = first.scheduled.find((run) => run.changes === 1)!;
  kit.pass("A run with nobody asking", `With nobody calling, notes.surface ran at ${new Date(ran.at).toISOString()} and surfaced "Water the plants"; "Renew the passport", due in an hour, stays put.`);

  // Only the timer: a scheduled mutation with no route refuses every caller (ADR-0017, update of 2026-11-08).
  await person.call("actions_invoke", { name: "notes.add", input: { body: "Call the plumber", remind_at: Date.now() - 1000 } });
  const byPerson = await kit.errorText(person.call("actions_invoke", { name: "notes.surface", input: {} }));
  assert.equal(byPerson, "Action notes.surface runs only on its schedule.");
  await waitFor("run that surfaced the second note", (current) => current.scheduled.filter((run) => run.changes === 1).length >= 2);
  const surfaced = (await person.call<{ body: string }[]>("actions_invoke", { name: "notes.inbox", input: {} })).map((note) => note.body).sort();
  assert.deepEqual(surfaced, ["Call the plumber", "Water the plants"]);
  kit.pass("Only the timer", `Rosa calling notes.surface herself is refused with "${byPerson}", and the statement has no actor clause to keep her out. The next run surfaces "Call the plumber".`);

  // Usage and Signals record each run.
  const seen = await usage();
  const total = seen.totals.find((row) => row.kind === "scheduled" && row.name === "notes.surface")?.count ?? 0;
  assert(total >= seen.scheduled.length && total >= 2, JSON.stringify(seen.totals));
  assert(seen.scheduled.every((run) => run.action === "notes.surface" && run.outcome === "ok"));
  const signals = await owner.call<{ signals: { name: string; properties: object }[] }>("signals_read", { deploymentId: app.deploymentId, name: "notes.surfaced" });
  assert.equal(signals.signals.length, 2);
  assert.deepEqual(signals.signals[0].properties, {});
  assert(!seen.totals.some((row) => row.kind === "refused"));
  kit.pass("Usage and Signals", `usage_read counts ${total} runs under scheduled and lists each with its outcome and changes, such as ${JSON.stringify(seen.scheduled.find((run) => run.changes === 0) ?? seen.scheduled[0])}. Only the 2 runs that changed a row recorded notes.surfaced.`);

  const notices = async () => (await readFile(path.join(kit.dataDir, "mailbox.jsonl"), "utf8").catch(() => ""))
    .trim().split("\n").filter(Boolean).map((line) => JSON.parse(line))
    .filter((mail) => mail.purpose === "notify" && mail.action === "notes.surface");
  for (const deadline = Date.now() + 10_000; (await notices()).length < 2 && Date.now() < deadline;) await Bun.sleep(100);
  const mail = await notices();
  assert.equal(mail.length, 2, "Each scheduled mutation that changed a row must queue its notice.");
  assert(mail.every((message) => message.to === "owner@probe.example" && message.subject === "Notes are due"));
  const noticeUsage = await usage();
  assert.equal(noticeUsage.totals.find((row) => row.kind === "notify" && row.name === "notes.surface: queued")?.count, 2);
  await waitFor("run with no new notes", (current) => current.scheduled[0].changes === 0);
  assert.equal((await notices()).length, 2, "A scheduled run that changes nothing sends no notice.");
  kit.pass("Scheduled owner notice", "The two runs that surfaced a note each emailed the owner through the notice queue. Usage counts two queued notices; runs that change no row send nothing.");

  // A failing run is logged and tried again at the next tick, and runs never pile up.
  await owner.call("database_execute", { deploymentId: app.deploymentId, operation: "sql", input: { sql: "ALTER TABLE notes RENAME TO notes_moved" } });
  const failing = await waitFor("second failed run", (current) => current.scheduled.filter((run) => run.outcome === "failed").length >= 2);
  const failure = failing.scheduled.find((run) => run.outcome === "failed")!;
  assert.match(failure.error ?? "", /no such table: notes/);
  await owner.call("database_execute", { deploymentId: app.deploymentId, operation: "sql", input: { sql: "ALTER TABLE notes_moved RENAME TO notes" } });
  const recovered = await waitFor("run after the table came back", (current) => current.scheduled[0].outcome === "ok");
  const failures = recovered.scheduled.filter((run) => run.outcome === "failed");
  const gaps = failures.slice(1).map((run, index) => failures[index].at - run.at);
  assert(gaps.every((gap) => gap >= 400), `Failed runs overlapped: ${gaps.join(", ")} ms apart.`);
  kit.pass("A failing run is retried", `With the table renamed, ${failures.length} runs failed one tick apart with "${failure.error}", none overlapping; the first tick after the owner renamed it back ran ok.`);

  // Maintenance runs nothing; leaving it runs the open slot once.
  await owner.call("migration_begin", { deploymentId: app.deploymentId, waitSeconds: 10 });
  const paused = (await usage()).scheduled.length;
  await Bun.sleep(5000);
  const during = (await usage()).scheduled.length;
  const status = await owner.call<{ operationId: string }>("migration_status", { deploymentId: app.deploymentId });
  await owner.call("migration_cancel", { operationId: status.operationId });
  await waitFor("run after maintenance", (current) => current.scheduled.length > during);
  assert(during - paused <= 1, `${during - paused} runs started during maintenance.`);
  assert.equal((await notices()).length, 2, "Failures, recovery without new rows and maintenance send no extra notice.");
  kit.pass("Maintenance", `While migration_begin held the app in maintenance for 5 seconds, ${during - paused} runs started (one may finish as the pause begins); after migration_cancel the next tick ran it again.`);
});
