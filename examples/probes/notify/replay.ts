// Probe: email on an event. Proves a notice to the owner when a visitor joins a waitlist, a confirmation to the person who
// ordered once their account proved its email, what is not sent (a retry that
// stored nothing, a guard's refusal, an unproven address, an address typed
// into the input), what the owner reads, and what both doors refuse.
//
//   KONAE_PROBE_PORT=23060 bun run examples/probes/notify/replay.ts
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { probe } from "../_kit";

const here = import.meta.dir;

await probe("notify", 23060, async (kit) => {
  const { owner } = kit;
  const app = await kit.publish(here, { projectId: "probe_notify", visibility: "public", signup: { mode: "open" } });
  await owner.call("database_execute", { deploymentId: app.deploymentId, operation: "sql", input: { sql: "INSERT INTO prints (id, title, stock) VALUES ('dune', 'Dune at dusk', 2)" } });
  const mailbox = async () => (await readFile(path.join(kit.dataDir, "mailbox.jsonl"), "utf8").catch(() => ""))
    .trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
  const notices = async () => (await mailbox()).filter((message) => message.purpose === "notify");
  const waitFor = async (count: number) => {
    let found = await notices();
    for (const deadline = Date.now() + 10_000; found.length < count && Date.now() < deadline; found = await notices()) await Bun.sleep(100);
    return found;
  };
  const invoke = (cookie: string | undefined, name: string, input: object) =>
    kit.runtime(app.host, cookie).call<any>("actions_invoke", { name, input });

  // 1. A visitor joins; the owner hears about it. The address the visitor
  // typed, with a header in it, is only text in the owner's email.
  const typed = "fan@probe.example\r\nBcc: victim@probe.example";
  assert.equal((await invoke(undefined, "waitlist.join", { email: typed, source: "newsletter" })).changes, 1);
  assert.equal((await invoke(undefined, "waitlist.join", { email: typed, source: "newsletter" })).changes, 0);
  const [first] = await waitFor(1);
  await Bun.sleep(300);
  assert.equal((await notices()).length, 1, "the second join stored nothing, so it sent nothing");
  assert.equal(first.to, "owner@probe.example");
  assert.equal(first.scope, `deployment:${app.deploymentId}`);
  assert.equal(first.action, "waitlist.join");
  assert.equal(first.subject, "New on the waitlist from newsletter");
  assert(first.text.startsWith("fan@probe.example Bcc: victim@probe.example joined the waitlist.\nThey came from newsletter.\n"), first.text);
  assert(first.text.includes("probe_notify sent this notice through Konae because you own the app."), first.text);
  assert(!(await mailbox()).some((message) => String(message.to).includes("victim")), "nobody the input names gets mail");
  kit.pass("Owner notice", `waitlist.join with "notify": { "to": "owner" } queues one email to the owner's account, "${first.subject}". The visitor's address with a Bcc line in it arrives as one line of text, "${first.text.split("\n")[0]}", and no address from the input receives anything. Joining again with the same address changes no row and sends nothing.`);

  // 2. The person who ordered gets a confirmation only once their account
  // has proved its email. In local mode signup proves nothing; an answered
  // recovery link does.
  const buyer = "buyer@probe.example";
  let cookie = await kit.signup(app.host, buyer);
  assert.equal((await invoke(cookie, "orders.place", { print: "dune" })).changes, 1);
  const recover = await kit.appFetch(app.host)(`${kit.url}/_konae/auth/recover`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email: buyer }),
  });
  assert.equal(recover.status, 202, await recover.clone().text());
  let link: any;
  for (const deadline = Date.now() + 10_000; !link && Date.now() < deadline; await Bun.sleep(100)) {
    link = (await mailbox()).find((message) => message.purpose === "recovery" && message.to === buyer);
  }
  assert(link, "the recovery link arrives in the development mailbox");
  const reset = await kit.appFetch(app.host)(`${kit.url}/_konae/auth/reset`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ token: new URL(link.link).hash.slice("#recovery=".length), password: crypto.randomUUID() }),
  });
  assert.equal(reset.status, 200, await reset.clone().text());
  cookie = reset.headers.get("set-cookie")!.split(";")[0];
  const session = await kit.runtime(app.host, cookie).call<{ userId: string }>("auth_session");
  assert.equal((await invoke(cookie, "orders.place", { print: "dune" })).changes, 1);
  const soldOut = await kit.errorText(invoke(cookie, "orders.place", { print: "dune" }));
  assert.equal(soldOut, "Sold out.");
  const all = await waitFor(2);
  await Bun.sleep(300);
  const toBuyer = (await notices()).filter((message) => message.to === buyer);
  assert.equal(all.length, 2);
  assert.equal(toBuyer.length, 1, "the unproven order and the refused one send nothing");
  assert.equal(toBuyer[0].subject, "Your order for dune");
  assert(toBuyer[0].text.startsWith(`We have your order for dune.\nYour account is ${session.userId}.\n`), toBuyer[0].text);
  kit.pass("Actor notice needs a proven email", `orders.place with "notify": { "to": "actor" } succeeds for a fresh local signup and sends nothing, because the account has not proved its address. After the buyer answers a recovery link, the next order sends "${toBuyer[0].subject}" to ${buyer}. The third order is refused with its guard, "${soldOut}", and sends nothing.`);

  // 3. The owner reads every outcome in Usage and the queue in mail_status.
  const today = new Date().toISOString().slice(0, 10);
  const usage = await owner.call<{ totals: Array<{ kind: string; name: string; count: number }> }>("usage_read", { deploymentId: app.deploymentId, from: today, to: today });
  const lines = usage.totals.filter((row) => row.kind === "notify").map((row) => `${row.name} = ${row.count}`);
  assert.deepEqual(lines, ["orders.place: queued = 1", "orders.place: unverified-email = 1", "waitlist.join: queued = 1"]);
  const status = await owner.call<any>("mail_status", {});
  assert.equal(status.queue.notices.delivered, 2);
  assert.equal(status.limits.notifyAppPerDay, 200);
  assert.equal(status.limits.notifyRecipientPerDay, 50);
  kit.pass("Owner reads the outcome", `usage_read lists ${lines.join(", ")} under the kind notify. mail_status counts ${status.queue.notices.delivered} delivered notices apart from account email, under budgets of ${status.limits.notifyAppPerDay} a day per app, ${status.limits.notifyCreatorPerDay} per creator and ${status.limits.notifyRecipientPerDay} per recipient.`);

  // 4. Both doors refuse the same declarations with the same sentence.
  const sources = { "schema.sql": await readFile(path.join(here, "schema.sql"), "utf8"), "actions.json": await readFile(path.join(here, "actions.json"), "utf8") };
  const program = JSON.parse(sources["actions.json"]);
  const variant = (change: (actions: any) => void) => {
    const copy = structuredClone(program);
    change(copy.actions);
    return JSON.stringify(copy);
  };
  const variants: Record<string, string> = {
    "notify on a query": variant((actions) => { actions["prints.list"].notify = actions["waitlist.join"].notify; }),
    "an address as the recipient": variant((actions) => { actions["waitlist.join"].notify.to = "launch@probe.example"; }),
    "the actor's email as a placeholder": variant((actions) => { actions["orders.place"].notify.body = "Sent to {actor.email}"; }),
    "a placeholder for an input the action does not bind": variant((actions) => { actions["orders.place"].notify.subject = "Order for {input.email}"; }),
    "a subject of 121 characters": variant((actions) => { actions["waitlist.join"].notify.subject = "x".repeat(121); }),
    "a changes column the table does not have": variant((actions) => { Object.assign(actions["waitlist.join"].notify, { changes: ["referrer"], body: "{changes}" }); }),
  };
  const answers: Record<string, string> = {};
  for (const [name, actions] of Object.entries(variants)) {
    const gate = await kit.gateRefusal({ ...sources, "actions.json": actions });
    const kernel = await kit.planRefusal(here, "probe_notify_refused", { "actions.json": actions });
    assert.equal(gate.detail, kernel, `${name}: the Gate and the Kernel must give the same sentence.`);
    answers[name] = kernel;
  }
  assert.equal(answers["notify on a query"], "Action prints.list is a query, so it cannot declare notify. Declare it on a mutation.");
  assert.equal(answers["an address as the recipient"], 'Action waitlist.join has an invalid notify recipient. Use "owner", "actor" or {"column": "<name>"} for a column of account IDs.');
  assert.equal(answers["the actor's email as a placeholder"], "Action orders.place names an unknown placeholder {actor.email} in its notify body. Use {input.<key>} for one of its input bindings, {actor.id}, {kernel.now}, {changes} or {changes.count}.");
  assert.equal(answers["a changes column the table does not have"], "Action waitlist.join names a notify column its statement cannot return. Name columns of the table the statement changes.");
  kit.pass("Refusals at both doors", `The Validation Gate and release_plan refuse with the same sentence: ${Object.entries(answers).map(([name, text]) => `${name}, "${text}"`).join("; ")}.`);
});
