// Replays the club showcase against two isolated Kernels: publish, invite,
// promote an organizer, store private and public files, recover a password,
// export the app as a package and import it on the second Kernel. Every
// assertion here is a claim the README makes.
import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { deflateSync } from "node:zlib";
import { chromium } from "playwright";
import { McpClient, checksum, encodeBase64, type Transfer } from "../../../src/mcp/client";
import { buildLocal, publishLocal } from "../../../scripts/littlebox";

const repo = path.resolve(import.meta.dir, "../../..");
const scratch = await mkdtemp(path.join(tmpdir(), "littlebox-showcase-club-"));
const portA = Number(process.env.LITTLEBOX_SHOWCASE_PORT ?? 19470);
const portB = portA + 1;
assert(Number.isInteger(portA) && portA >= 1024 && portB <= 65535, "Choose a valid local port.");
const password = crypto.randomUUID();
const newPassword = crypto.randomUUID();
const shots = process.env.LITTLEBOX_SHOWCASE_SCREENSHOTS;
const observations: { scenario: string; result: string }[] = [];
const refusals: { primitive: string; request: string; answer: string }[] = [];
const kernels: ReturnType<typeof Bun.spawn>[] = [];
let address = 0;
const freshAddress = () => `198.18.1.${++address % 250}`;
const json = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));

function check(scenario: string, result: string) {
  observations.push({ scenario, result });
  console.log(`PASS ${scenario}. ${result}`);
}
function refused(primitive: string, request: string, answer: string) {
  refusals.push({ primitive, request, answer });
}
async function message(promise: Promise<unknown>) {
  try { await promise; } catch (error) { return error instanceof Error ? error.message : String(error); }
  assert.fail("Expected a refusal.");
}

// Small real PNG covers, so the browser renders something a person would see.
function crc32(bytes: Uint8Array) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    let c = (crc ^ byte) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = (crc >>> 8) ^ c;
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function png(width: number, height: number, paint: (x: number, y: number) => [number, number, number]) {
  const raw = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) raw.set(paint(x, y), y * (width * 3 + 1) + 1 + x * 3);
  const chunk = (type: string, data: Buffer) => {
    const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
    const out = Buffer.alloc(body.length + 8);
    out.writeUInt32BE(data.length, 0); body.copy(out, 4); out.writeUInt32BE(crc32(body), body.length + 4);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0); header.writeUInt32BE(height, 4); header.set([8, 2, 0, 0, 0], 8);
  return new Uint8Array(Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", header), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]));
}
const cover = (base: [number, number, number], band: [number, number, number]) => png(120, 180, (x, y) =>
  x < 10 ? [base[0] * 0.6, base[1] * 0.6, base[2] * 0.6] as [number, number, number] : y > 60 && y < 96 ? band : base);
const coverOne = cover([88, 135, 235], [200, 243, 74]);
const coverTwo = cover([244, 93, 53], [241, 239, 231]);

async function startKernel(port: number, name: string) {
  const url = `http://127.0.0.1:${port}`;
  const dataDir = path.join(scratch, name);
  const setupKey = crypto.randomUUID();
  const kernel = Bun.spawn([path.join(repo, "kernel-rs/target/debug/littlebox-kernel")], {
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
  kernels.push(kernel);
  let ready = false;
  for (let attempt = 0; attempt < 100 && !ready; attempt++) {
    assert.equal(kernel.exitCode, null, `Kernel ${name} stopped before startup. Is port ${port} free?`);
    try { ready = (await fetch(`${url}/_littlebox/health`)).ok; } catch {}
    if (!ready) await Bun.sleep(100);
  }
  assert(ready, `Kernel ${name} did not start on port ${port}.`);
  const signup = await fetch(`${url}/_littlebox/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: `owner@${name}.example.test`, password, setupKey }),
  });
  assert.equal(signup.status, 200, await signup.clone().text());
  const cookie = signup.headers.get("set-cookie")!.split(";")[0];
  const owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  return { url, owner, mailbox: path.join(dataDir, "mailbox.jsonl") };
}
type Kernel = Awaited<ReturnType<typeof startKernel>>;
type Receipt = { id: string; hostname: string; url: string };

/** One app on one Kernel, with helpers that speak to its host as a browser would. */
function app(kernel: Kernel, receipt: Receipt) {
  const http = (route: string, init: RequestInit & { cookie?: string; bearer?: string } = {}) => {
    const headers = new Headers(init.headers);
    headers.set("host", receipt.hostname);
    headers.set("x-forwarded-for", freshAddress());
    if (init.cookie) headers.set("cookie", init.cookie);
    if (init.bearer) headers.set("authorization", `Bearer ${init.bearer}`);
    return fetch(kernel.url + route, { ...init, headers, redirect: "manual" });
  };
  const post = (route: string, body: object) => http(route, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const runtime = (cookie?: string) => new McpClient(`${kernel.url}/_littlebox/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("host", receipt.hostname);
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  const cookieOf = (response: Response) => response.headers.get("set-cookie")!.split(";")[0];
  const login = async (email: string, secret: string) => {
    const response = await post("/_littlebox/auth/login", { email, password: secret });
    return { status: response.status, body: await response.json().catch(() => ({})), cookie: response.ok ? cookieOf(response) : undefined };
  };
  return { receipt, http, post, runtime, cookieOf, login };
}

/** Waits for the newest development email of one purpose to one address and returns its token. */
async function mail(kernel: Kernel, to: string, purpose: string, after = 0) {
  for (const deadline = Date.now() + 10_000; Date.now() < deadline; await Bun.sleep(100)) {
    const lines = (await readFile(kernel.mailbox, "utf8").catch(() => "")).trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
    const matches = lines.filter((item) => item.to === to && item.purpose === purpose);
    if (matches.length > after) {
      const hash = new URL(matches.at(-1).link).hash;
      return { token: hash.slice(hash.indexOf("=") + 1), link: matches.at(-1).link as string, count: matches.length };
    }
  }
  assert.fail(`No ${purpose} email reached ${to}.`);
}

const action = <T = any>(client: McpClient, name: string, input: object = {}) => client.call<T>("actions_invoke", { name, input });
async function upload(client: McpClient, bytes: Uint8Array, name: string, type: string, visibility?: string) {
  const transfer = await client.upload(bytes);
  try { return await client.call<{ id: string; visibility: string }>("files_upload", { transferId: transfer.transferId, name, type, ...(visibility ? { visibility } : {}) }); }
  finally { await client.cancel(transfer.transferId).catch(() => {}); }
}
/** A package component upload names its component, which McpClient.upload cannot. */
async function uploadComponent(client: McpClient, bytes: Uint8Array, component: string, projectId: string) {
  const transfer = await client.call<Transfer>("transfer_begin", { projectId, size: bytes.length, sha256: await checksum(bytes), component });
  for (let offset = 0; offset < bytes.length; offset += transfer.chunkBytes) {
    await client.call("transfer_write", { transferId: transfer.transferId, offset, data: encodeBase64(bytes.subarray(offset, offset + transfer.chunkBytes)) });
  }
  await client.call("transfer_complete", { transferId: transfer.transferId });
  return transfer.transferId;
}
function cli(args: string[]) {
  const child = Bun.spawn(["bun", "run", path.join(repo, "scripts/littlebox.ts"), ...args], { cwd: repo, stdout: "pipe", stderr: "pipe" });
  return Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
}

try {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
  assert.equal(await build.exited, 0, "The Kernel must compile.");
  const origin = await startKernel(portA, "origin");
  const destination = await startKernel(portB, "destination");

  // Publish with the CLI's own publish path and a delegated token, as a creator would.
  const root = path.join(scratch, "club");
  await cp(import.meta.dir, root, { recursive: true, filter: (source) => !source.endsWith("replay.ts") && !source.endsWith("README.md") });
  const project = { id: "showcase_club", name: "Club", slug: "club", manifest: { version: 1, visibility: "private" } };
  await writeFile(path.join(root, "littlebox.json"), JSON.stringify({ url: origin.url, project }, null, 2));
  const publishToken = await origin.owner.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish"], ttlSeconds: 600 });
  const receiptA = await publishLocal(root, { url: origin.url, project }, publishToken.token) as Receipt;
  const A = app(origin, receiptA);
  const firstPolicy = await origin.owner.call<{ mode: string }>("signup_policy_read", { deploymentId: receiptA.id });
  assert.equal(firstPolicy.mode, "closed");
  await assert.rejects(new McpClient(`${origin.url}/mcp`, { token: publishToken.token }).call("signup_policy_set", { deploymentId: receiptA.id, mode: "invitation" }), /Unknown tool/);
  refused("Signup policy", "CLI publish of a private app, then signup_policy_set with the publish token", "Policy starts Closed; the publish token gets `Unknown tool or insufficient permission.`");
  await origin.owner.call("signup_policy_set", { deploymentId: receiptA.id, mode: "invitation" });
  check("Publish", `littlebox publish deployed ${receiptA.url} as private with policy ${firstPolicy.mode}; the owner session switched it to invitation.`);

  // A visitor gets the sign-in page and nothing else.
  const shell = await A.http("/");
  const shellText = await shell.text();
  assert.equal(shell.status, 200);
  assert.match(shellText, /<title>Sign in<\/title>/);
  assert.match(shellText, /Only invited people can create an account/);
  assert(!shellText.includes("/_littlebox/auth/signup"));
  const stranger = await A.post("/_littlebox/auth/signup", { email: "stranger@example.test", password });
  assert.equal(stranger.status, 403);
  refused("Invitation only", "POST /_littlebox/auth/signup from an uninvited address", `${stranger.status} ${JSON.stringify(await stranger.json())}`);
  const visitorList = await A.http("/_littlebox/mcp", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }) });
  assert.equal(visitorList.status, 401);
  check("Visitor", "GET / answers the sign-in page without a signup form, signup answers 403 and the runtime endpoint answers 401.");

  // Invitations through the development mailbox. Addresses come back lowercased.
  const people = { ana: "ana@club.example.test", ben: "ben@club.example.test", cy: "cy@club.example.test" };
  const invited = await origin.owner.call<{ results: Array<{ email: string; status: string; delivery: string }> }>("app_invitations_create", {
    deploymentId: receiptA.id, emails: Object.values(people).map((email) => email.replace("ana", "Ana")), idempotencyKey: "club-invites-0001",
  });
  assert.deepEqual(invited.results.map((item) => item.status), ["invited", "invited", "invited"]);
  const cookies: Record<keyof typeof people, string> = {} as never;
  for (const [who, email] of Object.entries(people) as [keyof typeof people, string][]) {
    const { token, link } = await mail(origin, email, "invitation");
    assert(link.includes("/_littlebox/invitation#invitation="));
    assert.deepEqual((await (await A.post("/_littlebox/auth/invitation", { token })).json()).result, { email, app: "Club", status: "new-account" });
    const accepted = await A.post("/_littlebox/auth/accept", { token, password });
    assert.equal(accepted.status, 200, await accepted.clone().text());
    cookies[who] = A.cookieOf(accepted);
  }
  const ana = A.runtime(cookies.ana), ben = A.runtime(cookies.ben), cy = A.runtime(cookies.cy);
  for (const [client, display] of [[ana, "Ana"], [ben, "Ben"], [cy, "Cy"]] as const) assert.equal((await action(client, "me.join", { display })).changes, 1);
  check("Invitations", "Three invitations went through the development mailbox; each link opened the acceptance flow and set a password.");

  // The owner promotes the organizer through owner SQL. The app's database
  // has no email column to match on, so the owner joins app_users_list by hand.
  const users = await origin.owner.call<{ users: Array<{ userId: string; email: string }> }>("app_users_list", { deploymentId: receiptA.id });
  const anaId = users.users.find((user) => user.email === people.ana)!.userId;
  await origin.owner.call("database_execute", { deploymentId: receiptA.id, operation: "sql", input: { sql: `UPDATE members SET role = 'organizer' WHERE user_id = '${anaId}'` } });
  assert.equal((await action(ana, "me.get"))[0].role, "organizer");
  assert.equal((await action(ben, "me.get"))[0].role, "member");
  check("Organizer", "The owner mapped Ana's email to her user ID with app_users_list and promoted her with database_execute.");

  // Files: a public cover each, and private notes.
  const anaCover = await upload(ana, coverOne, "cover-dispossessed.png", "image/png", "public");
  const benCover = await upload(ben, coverTwo, "cover-solaris.png", "image/png", "public");
  assert.equal((await action(ana, "books.propose", { title: "The Dispossessed", author: "Ursula K. Le Guin", coverFileId: anaCover.id })).changes, 1);
  assert.equal((await action(ben, "books.propose", { title: "Solaris", author: "Stanislaw Lem", coverFileId: benCover.id })).changes, 1);
  const books = await action<Array<{ id: number; title: string }>>(ana, "books.list");
  const dispossessed = books.find((book) => book.title === "The Dispossessed")!.id;
  const solaris = books.find((book) => book.title === "Solaris")!.id;
  const anaNoteText = "Shevek's wall: ask the group whether Anarres is free or only poor.";
  const benNoteText = "I skipped chapter four. Do not tell Ana.";
  const anaNote = await upload(ana, new TextEncoder().encode(anaNoteText), "note-the-dispossessed.txt", "text/plain");
  const benNote = await upload(ben, new TextEncoder().encode(benNoteText), "note-the-dispossessed.txt", "text/plain");
  assert.equal(anaNote.visibility, "private");
  assert.equal((await action(ana, "notes.add", { bookId: dispossessed, fileId: anaNote.id, name: "note-the-dispossessed.txt" })).changes, 1);
  assert.equal((await action(ben, "notes.add", { bookId: dispossessed, fileId: benNote.id, name: "note-the-dispossessed.txt" })).changes, 1);

  const benReadsAnaNote = await message(ben.call("files_read", { fileId: anaNote.id }));
  assert.equal(benReadsAnaNote, "File not found.");
  const benGetsAnaNote = await A.http(`/_littlebox/files/${anaNote.id}`, { cookie: cookies.ben });
  assert.equal(benGetsAnaNote.status, 404);
  refused("Private file", "Ben's files_read of Ana's note", benReadsAnaNote);
  refused("Private file", "Ben's GET /_littlebox/files/<Ana's note> with his session", `${benGetsAnaNote.status} ${await benGetsAnaNote.text()}`);
  const anaGetsNote = await A.http(`/_littlebox/files/${anaNote.id}`, { cookie: cookies.ana });
  assert.equal(anaGetsNote.status, 200);
  assert.match(anaGetsNote.headers.get("content-disposition") ?? "", /^attachment/);
  assert.equal(await anaGetsNote.text(), anaNoteText);
  const benGetsCover = await A.http(`/_littlebox/files/${anaCover.id}`, { cookie: cookies.ben });
  assert.equal(benGetsCover.status, 200);
  assert.equal(benGetsCover.headers.get("content-type"), "image/png");
  assert.match(benGetsCover.headers.get("content-disposition") ?? "", /^inline/);
  assert.deepEqual(new Uint8Array(await benGetsCover.arrayBuffer()), coverOne);
  assert.deepEqual(await ben.download(await ben.call<Transfer>("files_read", { fileId: anaCover.id })), coverOne);
  const benList = (await ben.call<Array<{ id: string }>>("files_list")).map((file) => file.id).sort();
  assert.deepEqual(benList, [anaCover.id, benCover.id, benNote.id].sort());
  const visitorCover = await A.http(`/_littlebox/files/${anaCover.id}`);
  assert.equal(visitorCover.status, 401);
  refused("Private Deployment", "A visitor's GET /_littlebox/files/<public cover>", `${visitorCover.status} ${await visitorCover.text()}`);
  // A visitor of a private app is asked to sign in before any file is looked
  // up, so a public cover, a private note and a missing ID answer alike.
  for (const id of [anaNote.id, crypto.randomUUID()]) {
    const visitorFile = await A.http(`/_littlebox/files/${id}`);
    assert.deepEqual([visitorFile.status, await visitorFile.json()], [401, { error: "authentication-required" }]);
  }
  // There is no visibility between "only me" and "every member". The runtime
  // tools refuse any other value, and so does the SDK (see Browser below).
  const sharedNote = await message(upload(ben, new TextEncoder().encode("For the organizers"), "for-organizers.txt", "text/plain", "organizers"));
  assert.equal(sharedNote, "Value is outside the allowed choices.");
  refused("File visibility", "files_upload with visibility \"organizers\" on the runtime endpoint", sharedNote);
  const sharedLater = await message(ben.call("files_update", { fileId: benNote.id, visibility: "organizers" }));
  assert.equal(sharedLater, "Value is outside the allowed choices.");
  refused("File visibility", "files_update of Ben's note with visibility \"organizers\"", sharedLater);
  // An organizer role lives in the app's rows; the Kernel knows only the uploader.
  const organizerDeletes = await message(ana.call("files_delete", { fileId: benCover.id }));
  assert.equal(organizerDeletes, "Only the person who uploaded this file may change or delete it.");
  refused("File lifecycle", "Ana (organizer) deletes Ben's public cover with files_delete", organizerDeletes);
  const anaDeletesBenNote = await message(ana.call("files_delete", { fileId: benNote.id }));
  assert.equal(anaDeletesBenNote, "File not found.");
  refused("File lifecycle", "Ana deletes Ben's private note with files_delete", anaDeletesBenNote);
  check("Files", "Ben cannot read Ana's private note over MCP or HTTP, reads her public cover both ways, and lists only what he may read. A visitor gets 401 for a public cover, a private note and a missing ID alike. Neither files_upload nor files_update accepts a visibility for organizers, and Ana's organizer role does not let her delete Ben's files.");

  // A program reads public covers with an API key scoped to public files.
  const key = await origin.owner.call<{ key: string; keyId: string }>("api_keys_create", { deploymentId: receiptA.id, name: "Cover mirror", files: ["public"] });
  const keyCover = await A.http(`/_littlebox/files/${benCover.id}`, { bearer: key.key });
  assert.equal(keyCover.status, 200);
  assert.deepEqual(new Uint8Array(await keyCover.arrayBuffer()), coverTwo);
  const keyNote = await A.http(`/_littlebox/files/${anaNote.id}`, { bearer: key.key });
  assert.equal(keyNote.status, 404);
  refused("API key", "A key scoped to public files reads a private note", `${keyNote.status} ${await keyNote.text()}`);
  const keyOnMcp = await A.http("/_littlebox/mcp", { method: "POST", bearer: key.key, headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }) });
  assert.equal(keyOnMcp.status, 401);
  refused("API key", "The same key on the runtime MCP endpoint", `${keyOnMcp.status} ${await keyOnMcp.text()}`);
  const keyForQuery = await message(origin.owner.call("api_keys_create", { deploymentId: receiptA.id, name: "Book list", actions: ["books.list"] }));
  refused("API key", "api_keys_create naming books.list, an authenticated action with no route", keyForQuery);
  check("API key", "A key scoped to public files reads a cover with Authorization: Bearer, gets 404 for a private note and 401 on the runtime endpoint.");

  // Roles in SQL: a member's organizer mutation changes nothing and raises nothing.
  assert.deepEqual(await action(ben, "books.schedule", { id: dispossessed, meetsOn: "2026-11-20" }), { changes: 0, lastInsertId: 0 });
  assert.deepEqual(await action(ben, "books.remove", { id: solaris }), { changes: 0, lastInsertId: 0 });
  assert.deepEqual(await action(ben, "books.schedule", { id: 999_999, meetsOn: "2026-11-20" }), { changes: 0, lastInsertId: 0 });
  assert.equal((await action(ben, "books.list")).length, 2);
  assert.equal((await action(ana, "books.schedule", { id: dispossessed, meetsOn: "2026-11-20" })).changes, 1);
  assert.equal((await action(ben, "books.finish", { id: dispossessed })).changes, 0);
  refused("Roles in SQL", "Ben (member) calls books.schedule, books.remove and books.finish", "`{ changes: 0, lastInsertId: 0 }`, the same answer as books.schedule on a book that does not exist");
  // Usage counts a zero-change mutation as a successful call.
  const today = new Date().toISOString().slice(0, 10);
  let scheduled = 0;
  for (const deadline = Date.now() + 5_000; scheduled < 3 && Date.now() < deadline; await Bun.sleep(100)) {
    const usage = await origin.owner.call<{ totals: Array<{ kind: string; name: string; count: number }> }>("usage_read", { deploymentId: receiptA.id, from: today, to: today });
    scheduled = usage.totals.find((total) => total.kind === "action" && total.name === "books.schedule")?.count ?? 0;
  }
  assert.equal(scheduled, 3);
  // An organizer is an app role; inviting stays with the Kernel owner.
  const organizerInvites = await message(ana.call("app_invitations_create", { deploymentId: receiptA.id, emails: ["dee@club.example.test"], idempotencyKey: "organizer-invite-01" }));
  refused("Roles in SQL", "Ana (organizer) calls app_invitations_create on the runtime endpoint", organizerInvites);
  refused("Usage", "usage_read after Ben's two refused schedules and Ana's one", `books.schedule counted ${scheduled} calls; nothing separates the two that changed nothing`);
  check("Roles", "Ben's schedule, remove and finish change zero rows, the same answer a missing book gets; Ana's schedule changes one.");

  if (process.env.LITTLEBOX_SHOWCASE_BROWSER !== "0") {
    // Ben signs in through the Kernel's own page, sees the covers from the
    // file read path, opens his note, and is refused an organizer action.
    const browser = await chromium.launch({ channel: "chrome" });
    try {
      let storage: Awaited<ReturnType<import("playwright").BrowserContext["storageState"]>> | undefined;
      for (const colorScheme of ["light", "dark"] as const) {
        const context = await browser.newContext({ colorScheme, viewport: { width: 1100, height: 1000 }, storageState: storage });
        const page = await context.newPage();
        const violations: string[] = [];
        page.on("console", (entry) => { if (/Content Security Policy|Refused to/i.test(entry.text())) violations.push(entry.text()); });
        page.on("pageerror", (error) => violations.push(`page error: ${error.message}`));
        await page.addInitScript(() => document.addEventListener("securitypolicyviolation", (event) =>
          console.error(`Refused to load ${event.blockedURI} by ${event.violatedDirective}`)));
        const response = await page.goto(receiptA.url, { waitUntil: "load" });
        assert.equal(response?.status(), 200);
        if (!storage) {
          await page.getByLabel("Email").fill(people.ben);
          await page.getByLabel("Password").fill(password);
          await page.getByRole("button", { name: "Sign in" }).click();
        }
        await page.locator("#club").waitFor({ state: "visible", timeout: 15_000 });
        assert(response?.headers()["content-security-policy"]?.includes("img-src 'self'"));
        await page.waitForFunction(() => {
          const images = [...document.querySelectorAll<HTMLImageElement>("img.cover")];
          return images.length === 2 && images.every((img) => img.complete && img.naturalWidth === 120);
        }, undefined, { timeout: 10_000 });
        await page.locator("#notes summary").first().click();
        await page.locator("#notes .note-text").filter({ hasText: benNoteText }).waitFor({ timeout: 10_000 });
        if (!storage) {
          // The SDK refuses a visibility the Kernel lacks instead of storing a private file.
          const sdk = await page.evaluate(async (noteId) => {
            const miniapp = (window as any).miniapp;
            const before = (await miniapp.files.list()).length;
            let refusal = "stored";
            try { await miniapp.files.upload(new File(["For the organizers"], "for-organizers.txt", { type: "text/plain" }), { visibility: "organizers" }); } catch (error) { refusal = (error as Error).message; }
            return { refusal, url: await miniapp.files.url(noteId), added: (await miniapp.files.list()).length - before };
          }, benNote.id);
          assert.deepEqual(sdk, { refusal: "File visibility must be private or public.", url: `/_littlebox/files/${benNote.id}`, added: 0 });
          refused("File visibility", "miniapp.files.upload(file, { visibility: \"organizers\" }) in Ben's page", sdk.refusal);
        }
        await page.locator('button[data-action="books.schedule"]').first().click();
        await page.locator("#status").filter({ hasText: "Only an organizer can schedule a meeting" }).waitFor({ timeout: 10_000 });
        assert.equal(await page.locator("#who-role").textContent(), "member");
        if (shots) {
          await mkdir(shots, { recursive: true });
          await page.screenshot({ path: path.join(shots, `club-${colorScheme}.png`), fullPage: true });
        }
        assert.deepEqual(violations, [], `The club page broke under the Content Security Policy (${colorScheme}).`);
        storage = await context.storageState();
        await context.close();
      }
    } finally { await browser.close(); }
    check("Browser", `Chrome signed Ben in through the Kernel's page, rendered both covers from /_littlebox/files, fetched his note from miniapp.files.url, saw the SDK refuse visibility "organizers" without storing a file, showed the organizer refusal, and reported no policy violation${shots ? `; screenshots in ${shots}` : ""}.`);
  }

  // Account recovery: the emailed link sets a new password and ends Cy's old session.
  assert.equal((await cy.call<{ name: string }>("auth_session")).name, people.cy);
  const recover = await A.post("/_littlebox/auth/recover", { email: people.cy });
  assert.equal(recover.status, 202);
  const unknownRecover = await A.post("/_littlebox/auth/recover", { email: "nobody@club.example.test" });
  assert.equal(unknownRecover.status, 202);
  const recovery = await mail(origin, people.cy, "recovery");
  assert(recovery.link.includes("/_littlebox/account#recovery="));
  const reset = await A.post("/_littlebox/auth/reset", { token: recovery.token, password: newPassword });
  assert.equal(reset.status, 200, await reset.clone().text());
  const cyNew = A.runtime(A.cookieOf(reset));
  const oldSession = await message(cy.call("auth_session"));
  assert.equal(oldSession, "Sign in to use Littlebox MCP.");
  refused("Recovery", "Cy's session from before the reset calls auth_session", oldSession);
  assert.equal((await cyNew.call<{ name: string }>("auth_session")).name, people.cy);
  const oldPassword = await A.login(people.cy, password);
  assert.notEqual(oldPassword.status, 200);
  refused("Recovery", "Cy signs in with the old password", `${oldPassword.status} ${JSON.stringify(oldPassword.body)}`);
  assert.equal((await A.login(people.cy, newPassword)).status, 200);
  const reused = await A.post("/_littlebox/auth/reset", { token: recovery.token, password: crypto.randomUUID() });
  assert.equal(reused.status, 410);
  refused("Recovery", "The same recovery link used twice", `${reused.status} ${await reused.text()}`);
  check("Recovery", "The emailed link reset Cy's password; her earlier session answers 401, the old password fails and the link works once.");

  // Export the app as a package from the origin Kernel.
  const exported = await origin.owner.call<{ manifest: any; downloads: Record<string, Transfer> }>("package_export", { deploymentId: receiptA.id });
  assert.equal(exported.manifest.visibility, "private");
  assert.deepEqual(exported.manifest.signupPolicy, { mode: "invitation", allowlist: [] });
  const componentBytes: Record<string, Uint8Array> = {};
  for (const content of exported.manifest.contents) componentBytes[content.name] = await origin.owner.download(exported.downloads[content.name]);
  check("Export", `package_export wrote ${exported.manifest.contents.map((c: any) => `${c.name} ${c.size} B`).join(", ")}; excluded: ${exported.manifest.excluded.join("; ")}.`);

  // Import into a second Kernel, under a second owner who never saw the origin.
  await destination.owner.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const components: Record<string, string> = {};
  for (const content of exported.manifest.contents) components[content.name] = await uploadComponent(destination.owner, componentBytes[content.name], content.name, project.id);
  const { artifact, files } = await buildLocal(root);
  const payload = { project: { ...project, files }, visibility: "private", artifact, initialPackage: { manifest: exported.manifest, components } };
  const payloadUpload = await destination.owner.upload(json(payload), project.id);
  let receiptB: Receipt;
  try {
    const plan = await destination.owner.call<{ planId: string }>("release_plan", { transferId: payloadUpload.transferId });
    receiptB = await destination.owner.call<Receipt>("release_apply", { planId: plan.planId });
  } finally {
    await destination.owner.cancel(payloadUpload.transferId).catch(() => {});
    for (const id of Object.values(components)) await destination.owner.cancel(id).catch(() => {});
  }
  const B = app(destination, receiptB);
  assert.equal((await destination.owner.call<{ mode: string }>("signup_policy_read", { deploymentId: receiptB.id })).mode, "invitation");
  const anaB = await B.login(people.ana, password);
  const benB = await B.login(people.ben, password);
  const cyB = await B.login(people.cy, newPassword);
  assert.deepEqual([anaB.status, benB.status, cyB.status], [200, 200, 200]);
  const anaOnB = B.runtime(anaB.cookie), benOnB = B.runtime(benB.cookie);
  assert.equal((await action(anaOnB, "me.get"))[0].role, "organizer");
  assert.deepEqual((await action(anaOnB, "books.list")).map((book: any) => [book.title, book.status, book.cover_file_id]),
    [["The Dispossessed", "current", anaCover.id], ["Solaris", "proposed", benCover.id]]);
  const coverOnB = await B.http(`/_littlebox/files/${anaCover.id}`, { cookie: benB.cookie });
  assert.equal(coverOnB.status, 200);
  assert.deepEqual(new Uint8Array(await coverOnB.arrayBuffer()), coverOne);
  assert.equal(new TextDecoder().decode(await anaOnB.download(await anaOnB.call<Transfer>("files_read", { fileId: anaNote.id }))), anaNoteText);
  assert.equal(await message(benOnB.call("files_read", { fileId: anaNote.id })), "File not found.");
  assert.equal((await B.http(`/_littlebox/files/${anaNote.id}`, { cookie: benB.cookie })).status, 404);
  assert.equal((await action(benOnB, "books.finish", { id: dispossessed })).changes, 0);
  assert.equal((await action(anaOnB, "books.finish", { id: dispossessed })).changes, 1);

  const originSession = await message(B.runtime(cookies.ana).call("auth_session"));
  assert.equal(originSession, "Sign in to use Littlebox MCP.");
  refused("Import", "Ana's origin session cookie on the destination Kernel", originSession);
  const originKey = await B.http(`/_littlebox/files/${anaCover.id}`, { bearer: key.key });
  const originKeyBody = await originKey.json();
  assert.deepEqual([originKey.status, originKeyBody], [401, { error: "invalid-api-key" }]);
  refused("Import", "The origin's API key on the destination's file read path", `${originKey.status} ${JSON.stringify(originKeyBody)}`);
  const keysOnB = await destination.owner.call<{ keys: Array<{ keyId: string; revokedAt: number | null }> }>("api_keys_list", { deploymentId: receiptB.id });
  // The package's identity component leaves API keys out entirely, so the
  // destination owner has no record of which keys existed at the origin.
  assert.deepEqual(keysOnB.keys, []);
  refused("Import", "api_keys_list on the destination after a package import", "`{ keys: [] }`: no metadata travels, revoked or not");
  const invitationsOnB = await destination.owner.call<{ invitations: Array<{ status: string }> }>("app_invitations_list", { deploymentId: receiptB.id });
  check("Import", `The destination serves ${receiptB.url} under invitation; Ana, Ben and Cy sign in with their passwords, the organizer role, rows, cover and note bytes are intact, Ben still cannot read Ana's note, origin sessions answer 401, the origin key answers invalid-api-key and the destination lists no keys. Invitation history travelled as ${invitationsOnB.invitations.map((item) => item.status).join(", ")}.`);

  // The Action Contract and the conformance checks, through the CLI, against both Kernels.
  const [contractCode, , contractErr] = await cli(["contract", "--dir", root]);
  assert.equal(contractCode, 0, contractErr);
  const conformTotals: Record<string, string> = {};
  for (const [label, target, cookie] of [["origin", A, (await A.login(people.ben, password)).cookie!], ["destination", B, benB.cookie!]] as const) {
    const token = cookie.split("=").slice(1).join("=");
    const [code, out, err] = await cli(["conform", "--url", target.receipt.url, "--contract", path.join(root, "action-contract.json"), "--token", token]);
    const totals = out.trim().split("\n").at(-1)!;
    assert.equal(code, 0, out + err);
    assert.match(totals, /^\d+ passed, 0 failed, 0 skipped/);
    conformTotals[label] = totals;
  }
  check("Action Contract", `littlebox contract wrote the contract for 10 actions; conform as Ben: origin "${conformTotals.origin}", destination "${conformTotals.destination}"`);

  console.log(JSON.stringify({ app: "club", checks: observations.length, observations, refusals, conform: conformTotals }, null, 2));
} finally {
  for (const kernel of kernels) kernel.kill();
  await Promise.all(kernels.map((kernel) => kernel.exited));
  await rm(scratch, { recursive: true, force: true });
}
