// Probe: a scheduled notice that reaches the people its rows name (ADR-0012),
// run on `konae dev`, the Kernel a person runs on an app folder on their own
// machine. Ana and Ben accept invitations the `kernel` binary's invite command
// sends, which proves their addresses.
// Ana assigns chores to herself, to Ben, to an email address and to an ID that
// is no account. The hourly chores.remind runs every 2 seconds here
// (KONAE_SCHEDULE_MIN_SECONDS), and the development mailbox shows one notice
// to Ana and one to Ben, each listing only their own chores.
//
//   KONAE_PROBE_PORT=20010 bun run examples/probes/notify-rows/replay.ts [--keep <folder>]
//
// dev serves its events page on the next port, so that one must be free too.
// KONAE_KERNEL_BIN names a built `kernel` binary, which dev then runs instead of compiling one.
// --keep copies the mailbox file to <folder>/mailbox.jsonl as evidence.
import assert from "node:assert/strict";
import { copyFile, cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../../../src/mcp/client";
import { appDir, localKernel } from "../../../scripts/konae";
import { KERNEL_VERSION, type AppSettings } from "../../../src/lib/app-folder";

const repo = path.resolve(import.meta.dir, "../../..");
const port = Number(process.env.KONAE_PROBE_PORT ?? 20010);
const url = `http://127.0.0.1:${port}`;
const keep = process.argv.includes("--keep") ? process.argv[process.argv.indexOf("--keep") + 1] : undefined;
const scratch = await mkdtemp(path.join(tmpdir(), "konae-notify-rows-"));
const mailboxFile = path.join(scratch, ".konae/kernel/mailbox.jsonl");
type Mail = { purpose: string; to: string; subject?: string; text?: string; link?: string; action?: string };
const mailbox = async (): Promise<Mail[]> => (await readFile(mailboxFile, "utf8").catch(() => ""))
  .trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
const waitFor = async <T>(what: string, find: () => Promise<T | undefined>, seconds = 20): Promise<T> => {
  for (const deadline = Date.now() + seconds * 1000; Date.now() < deadline; await Bun.sleep(200)) {
    const found = await find();
    if (found !== undefined) return found;
  }
  throw Error(`No ${what} within ${seconds} seconds.`);
};

let dev: ReturnType<typeof Bun.spawn> | undefined;
try {
  try { await fetch(`${url}/_konae/health`); throw Error(`Something already answers on port ${port}. Set KONAE_PROBE_PORT.`); }
  catch (error) { if (error instanceof Error && error.message.startsWith("Something")) throw error; }
  await mkdir(appDir(scratch), { recursive: true });
  for (const file of ["index.html", "schema.sql", "actions.json"]) await cp(path.join(import.meta.dir, file), path.join(appDir(scratch), file));
  // The app starts Invitation only, as konae init writes it.
  const settings: AppSettings = {
    id: "probe_notify_rows", name: "Chores", slug: "chores", visibility: "private",
    signupPolicy: { mode: "invitation", allowlist: [] }, signInMethods: ["password"], kernel: KERNEL_VERSION, manifest: { version: 1 },
  };
  await writeFile(path.join(scratch, "konae.json"), `${JSON.stringify(settings, null, 2)}\n`);

  const kernelBin = process.env.KONAE_KERNEL_BIN;
  dev = Bun.spawn(["bun", "run", path.join(repo, "scripts/konae.ts"), "dev", "--dir", scratch, "--port", String(port)], {
    cwd: repo, stdout: "pipe", stderr: "inherit",
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))),
      KONAE_SCHEDULE_MIN_SECONDS: "2", ...(kernelBin ? { KONAE_KERNEL_BIN: kernelBin } : {}),
    },
  });
  const output = dev.stdout.getReader();
  let printed = "";
  const appUrl = await waitFor("app URL from konae dev", async () => {
    const { value, done } = await output.read();
    if (done) throw Error(`konae dev stopped:\n${printed}`);
    printed += new TextDecoder().decode(value);
    return printed.match(/^(http:\/\/\S+)$/m)?.[1];
  }, 300);
  const host = new URL(appUrl).host;
  // The same binary and environment dev runs, for the operator commands.
  const kernel = await localKernel(scratch, settings, { port: String(port), build: false });
  const operator = async <T>(args: string[]) => JSON.parse(await kernel.run(args)) as T;

  let address = 0;
  const appFetch = (cookie?: string): typeof fetch => (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("host", host);
    headers.set("x-forwarded-for", `198.18.0.${++address}`);
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  };
  // Ana and Ben accept invitations the operator sends, which proves their addresses.
  const member = async (email: string) => {
    const invited = await operator<{ results: { status: string }[] }>(["invite", email]);
    assert.equal(invited.results[0].status, "invited");
    const invitation = await waitFor(`invitation for ${email}`, async () => (await mailbox()).find((mail) => mail.purpose === "invitation" && mail.to === email));
    const accepted = await appFetch()(`${url}/_konae/auth/accept`, {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ token: new URL(invitation.link!).hash.slice("#invitation=".length), password: crypto.randomUUID() }),
    });
    assert.equal(accepted.status, 200, await accepted.clone().text());
    const app = new McpClient(`${url}/_konae/mcp`, { fetch: appFetch(accepted.headers.get("set-cookie")!.split(";")[0]) });
    const session = await app.call<{ userId: string }>("auth_session");
    return { app, id: session.userId };
  };
  const ana = await member("ana@chores.test");
  const ben = await member("ben@chores.test");

  const soon = Date.now() + 3_600_000;
  const assign = (title: string, assignee: string, due = soon) => ana.app.call("actions_invoke", { name: "chores.assign", input: { title, assignee_id: assignee, due_at: due } });
  await assign("Water the plants", ana.id);
  await assign("Take out the bins", ben.id);
  await assign("Fix the bike", ben.id);
  await assign("Pay the rent", "cy@chores.test");
  await assign("Feed the cat", "user_that_does_not_exist");
  await assign("Paint the fence", ana.id, Date.now() + 3 * 86_400_000);

  const notices = await waitFor("two notices", async () => {
    const found = (await mailbox()).filter((mail) => mail.purpose === "notify");
    return found.length >= 2 ? found : undefined;
  });
  await Bun.sleep(4000);
  const settled = (await mailbox()).filter((mail) => mail.purpose === "notify");
  assert.equal(settled.length, 2, `Only Ana and Ben get a notice, once: ${JSON.stringify(settled)}`);
  const to = (email: string) => notices.find((mail) => mail.to === email)!;
  assert.equal(to("ana@chores.test").subject, "1 chores due");
  assert.match(to("ana@chores.test").text!, /^Due within a day of \d{4}-\d\d-\d\d \d\d:\d\d UTC:\n- Water the plants\n\n-- \n/);
  assert.equal(to("ben@chores.test").subject, "2 chores due");
  assert.match(to("ben@chores.test").text!, /^Due within a day of \d{4}-\d\d-\d\d \d\d:\d\d UTC:\n- Take out the bins\n- Fix the bike\n\n-- \n/);
  assert(!to("ben@chores.test").text!.includes("Water the plants"), "Ben reads only his own chores.");
  assert(notices.every((mail) => mail.action === "chores.remind"));

  const today = new Date().toISOString().slice(0, 10);
  // The running Kernel writes usage in the background, so the read waits for both notice lines.
  type Events = { summary: { usage: { totals: { kind: string; name: string; count: number }[] } } };
  const lines = await waitFor("notice usage", async () => {
    const events = await operator<Events>(["events", "--from", today, "--to", today]);
    const found = Object.fromEntries(events.summary.usage.totals.filter((row) => row.kind === "notify").map((row) => [row.name, row.count]));
    return Object.keys(found).length >= 2 ? found : undefined;
  });
  assert.deepEqual(lines, { "chores.remind: queued": 2, "chores.remind: not-a-verified-account": 2 });
  const mine = await ana.app.call<{ title: string; reminded_at: number | null }[]>("actions_invoke", { name: "chores.mine", input: {} });
  assert.deepEqual(mine.map((chore) => [chore.title, chore.reminded_at !== null]), [["Water the plants", true], ["Paint the fence", false]]);

  if (keep) await copyFile(mailboxFile, path.join(keep, "mailbox.jsonl"));
  console.log(`PASS A scheduled notice reaches the members its rows name. On konae dev, chores.remind ran with nobody calling and wrote two notices to ${mailboxFile}: "${to("ana@chores.test").subject}" to ana@chores.test and "${to("ben@chores.test").subject}" to ben@chores.test, each listing only that member's chores. The chores assigned to cy@chores.test and to an ID that is no account sent nothing, and kernel events says ${JSON.stringify(lines)}.`);
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  dev?.kill("SIGTERM");
  await dev?.exited;
  await rm(scratch, { recursive: true, force: true });
}
