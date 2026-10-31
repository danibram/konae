import assert from "node:assert/strict";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { chromium } from "playwright";
import { McpClient } from "../../src/mcp/client";
import { signRoute } from "../../src/lib/route-signature";
import { buildLocal, publishLocal } from "../../scripts/konae";
import { kernelTarget } from "../../scripts/conform";
import { generateActionContract } from "../../src/lib/action-contract";
import { formatConformanceReport, runConformance } from "../../src/lib/action-conformance";

const repo = path.resolve(import.meta.dir, "../..");
const scratch = await mkdtemp(path.join(tmpdir(), "konae-product-fit-"));
const port = Number(process.env.KONAE_PRODUCT_FIT_PORT ?? 19410);
assert(Number.isInteger(port) && port >= 1024 && port <= 65535, "Choose a valid local port.");
const url = `http://127.0.0.1:${port}`;
const setupKey = crypto.randomUUID();
const password = crypto.randomUUID();
// The feed's monitors sign their pings with this secret; the owner sets it before publishing.
const pingsSecret = crypto.randomUUID();
const badgePng = Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a8ioAAAAASUVORK5CYII="), (c) => c.charCodeAt(0));
const observations: { scenario: string; result: string }[] = [];
let kernel: ReturnType<typeof Bun.spawn> | undefined;
let address = 0;
const freshAddress = () => `198.18.0.${++address}`;
const bytes = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));
function check(scenario: string, result: string) {
  observations.push({ scenario, result });
  console.log(`PASS ${scenario}. ${result}`);
}

try {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
  assert.equal(await build.exited, 0, "The Kernel must compile.");
  kernel = Bun.spawn([path.join(repo, "kernel-rs/target/debug/konae-kernel")], {
    cwd: repo,
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))),
      KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_PUBLIC_ORIGIN: url,
      KONAE_HOST_SUFFIX: "localhost", KONAE_DATA_DIR: path.join(scratch, "data"),
      KONAE_ADMIN_KEY: setupKey, KONAE_BUN_PATH: process.execPath, KONAE_SOURCE_ROOT: repo,
      KONAE_TRUSTED_PROXIES: "127.0.0.1",
    },
    stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    assert.equal(kernel.exitCode, null, "The isolated Kernel stopped before startup.");
    try { ready = (await fetch(`${url}/_konae/health`)).ok; } catch {}
    if (ready) break;
    await Bun.sleep(100);
  }
  assert(ready, "The isolated Kernel did not start.");
  const signup = await fetch(`${url}/_konae/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@example.test", password, setupKey }),
  });
  assert.equal(signup.status, 200, await signup.clone().text());
  const ownerCookie = signup.headers.get("set-cookie")!.split(";")[0];
  const owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", ownerCookie);
    return fetch(input, { ...init, headers });
  } });
  const receipts: Record<string, { id: string; hostname: string; url: string }> = {};
  for (const name of ["launch", "journal", "board", "portfolio", "poll", "feed", "docs", "probe"]) {
    const root = path.join(scratch, name);
    await cp(path.join(import.meta.dir, name), root, { recursive: true });
    const project = { id: `product_fit_${name}`, name, slug: name, manifest: { version: 1, visibility: name === "board" ? "private" : "public" } };
    await owner.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
    if (name === "feed") await owner.call("secret_set", { projectId: project.id, name: "pings_webhook", value: pingsSecret });
    const compiled = await buildLocal(root);
    const upload = await owner.upload(bytes({ project: { ...project, files: compiled.files }, visibility: project.manifest.visibility, artifact: compiled.artifact }), project.id);
    try {
      const plan = await owner.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
      receipts[name] = await owner.call("release_apply", { planId: plan.planId });
    } finally { await owner.cancel(upload.transferId); }
  }
  check("Publish eight examples", "All builds pass MCP release planning and apply on an empty Kernel.");
  // The feed embeds a badge from the file read path. A reporter uploads it as
  // public and the owner points the page at it, before the browser pass loads it.
  const reporterSignup = await fetch(`${url}/_konae/auth/signup`, {
    method: "POST", headers: { host: receipts.feed.hostname, "content-type": "application/json", "x-forwarded-for": freshAddress() },
    body: JSON.stringify({ email: "reporter@example.test", password }),
  });
  assert.equal(reporterSignup.status, 200, await reporterSignup.clone().text());
  const reporterCookie = reporterSignup.headers.get("set-cookie")!.split(";")[0];
  const reporter = new McpClient(`${url}/_konae/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", receipts.feed.hostname); headers.set("cookie", reporterCookie);
    return fetch(input, { ...init, headers });
  } });
  const uploadFile = async (visibility: "public" | "private") => {
    const transfer = await reporter.upload(badgePng);
    try { return await reporter.call<{ id: string }>("files_upload", { transferId: transfer.transferId, name: `badge-${visibility}.png`, type: "image/png", visibility }); }
    finally { await reporter.cancel(transfer.transferId); }
  };
  const publicBadge = await uploadFile("public");
  const privateBadge = await uploadFile("private");
  await owner.call("database_execute", { deploymentId: receipts.feed.id, operation: "sql", input: { sql: `INSERT INTO badge(file_id) VALUES('${publicBadge.id}')` } });

  if (process.env.KONAE_PRODUCT_FIT_BROWSER !== "0") {
    // Each published page runs in Chrome under the Kernel's Content Security
    // Policy. A blocked script, style, image or request fails the replay.
    const browser = await chromium.launch({ channel: "chrome" });
    try {
      const pages: [string, string][] = Object.keys(receipts).flatMap(name => name === "docs"
        ? [[name, "/"], [name, "/guide/install.html"]] as [string, string][] : [[name, "/"]] as [string, string][]);
      for (const [name, route] of pages) {
        const context = await browser.newContext();
        const page = await context.newPage();
        const violations: string[] = [];
        page.on("console", message => { if (/Content Security Policy|Refused to/i.test(message.text())) violations.push(message.text()); });
        page.on("pageerror", error => violations.push(`page error: ${error.message}`));
        await page.addInitScript(() => document.addEventListener("securitypolicyviolation", event =>
          console.error(`Refused to load ${event.blockedURI} by ${event.violatedDirective}`)));
        const response = await page.goto(new URL(route, receipts[name].url).href, { waitUntil: "load" });
        assert.equal(response?.status(), 200, `${name}${route} did not load.`);
        assert(response?.headers()["content-security-policy"]?.includes("frame-ancestors 'none'"), `${name}${route} has no policy.`);
        if (name === "board") {
          // The private sign-in page reports the refusal only if its inline script runs.
          await page.getByLabel("Email").fill("stranger@example.test");
          await page.getByLabel("Password").fill(password);
          await page.getByRole("button", { name: "Sign in" }).click();
          await page.locator("#error").filter({ hasText: /\S/ }).waitFor({ timeout: 10_000 });
        } else {
          await page.waitForFunction(() => typeof (window as any).miniapp === "object", undefined, { timeout: 10_000 });
          await page.waitForTimeout(1_500);
        }
        if (name === "feed") {
          // The image comes from GET /_konae/files/<id> under the app's own policy.
          await page.waitForFunction(() => { const img = document.querySelector<HTMLImageElement>("#badge img"); return !!img && img.complete && img.naturalWidth > 0; }, undefined, { timeout: 10_000 });
        }
        assert.deepEqual(violations, [], `${name}${route} broke under the Content Security Policy.`);
        await context.close();
      }
    } finally { await browser.close(); }
    check("Content Security Policy", "Every published example loads in Chrome, runs its inline runtime and module, and reports no policy violation.");
  }


  function runtime(name: string, cookie?: string) {
    return new McpClient(`${url}/_konae/mcp`, { fetch: (input, init) => {
      const headers = new Headers(init?.headers); headers.set("host", receipts[name].hostname);
      if (cookie) headers.set("cookie", cookie);
      return fetch(input, { ...init, headers });
    } });
  }
  async function signupApp(name: string, email: string, expected = 200) {
    const response = await fetch(`${url}/_konae/auth/signup`, {
      method: "POST", headers: { host: receipts[name].hostname, "content-type": "application/json", "x-forwarded-for": freshAddress() },
      body: JSON.stringify({ email, password }),
    });
    assert.equal(response.status, expected, await response.clone().text());
    return runtime(name, response.headers.get("set-cookie")?.split(";")[0]);
  }
  const action = <T = any>(client: McpClient, name: string, input: object = {}) => client.call<T>("actions_invoke", { name, input });
  const sql = (name: string, statement: string) => owner.call("database_execute", { deploymentId: receipts[name].id, operation: "sql", input: { sql: statement } });
  const getPage = (name: string, route = "/") => fetch(url + route, { headers: { host: receipts[name].hostname }, redirect: "manual" });

  const launch = runtime("launch");
  assert.equal((await action(launch, "waitlist.join", { email: "visitor@example.test", name: "Visitor", source: "smoke" })).changes, 1);
  assert.equal((await action(launch, "waitlist.count"))[0].n, 1);
  // The statement stores the address trimmed and lowercased, and its guard refuses a repeat in any case.
  await assert.rejects(action(launch, "waitlist.join", { email: "visitor@example.test", name: "Visitor", source: "smoke" }), /That address is already on the list\./);
  await assert.rejects(action(launch, "waitlist.join", { email: " Visitor@Example.TEST", name: "Visitor", source: "smoke" }), /That address is already on the list\./);
  assert.equal((await action(launch, "waitlist.count"))[0].n, 1);
  check("Anonymous waitlist", "A signed-out visitor joins once, and the same address again, in any case, is refused with the guard's sentence.");

  const ana = await signupApp("journal", "ana@example.test");
  const bob = await signupApp("journal", "bob@example.test");
  assert.equal((await action(ana, "entries.create", { body: "Ana's private entry", mood: "calm" })).changes, 1);
  const entry = (await action(ana, "entries.list"))[0];
  assert.equal(entry.body, "Ana's private entry");
  assert.deepEqual(await action(bob, "entries.list"), []);
  await assert.rejects(action(bob, "entries.delete", { id: entry.id }), /This entry is already deleted, or it is not yours\./);
  assert.equal((await action(ana, "entries.list")).length, 1);
  await assert.rejects(action(runtime("journal"), "entries.list"), /authentication-required/);
  check("Private journal rows", "A second account cannot read the first account's entry, and its delete is refused with the guard.");

  assert.match(await (await getPage("board")).text(), /<title>Sign in<\/title>/);
  await owner.call("signup_policy_set", { deploymentId: receipts.board.id, mode: "allowlist", allowlist: ["@team.example"] });
  const lead = await signupApp("board", "lead@team.example");
  const member = await signupApp("board", "member@team.example");
  const dev = await signupApp("board", "dev@team.example");
  await signupApp("board", "stranger@example.test", 403);
  for (const [client, display] of [[lead, "Lead"], [member, "Member"], [dev, "Developer"]] as const) await action(client, "me.join", { display });
  await sql("board", "UPDATE members SET role='admin' WHERE display='Lead'");
  await action(member, "cards.create", { title: "Only my owner or an admin can move me" });
  const card = (await action(member, "cards.list"))[0];
  // cards.list names the owner by display name and carries no user ID to the browser.
  assert.deepEqual(Object.keys(card).sort(), ["id", "owner", "status", "title"]);
  const memberRows = await action<Array<{ id: number; display: string }>>(dev, "members.list");
  const idOf = (display: string) => memberRows.find((row) => row.display === display)!.id;
  await assert.rejects(action(dev, "cards.move", { id: card.id, status: "done" }), /Only the card's owner or an admin can move it/);
  await assert.rejects(action(dev, "cards.delete", { id: card.id }), /Only an admin can delete a card\./);
  await assert.rejects(action(dev, "members.promote", { id: idOf("Developer") }), /Only an admin can promote a member/);
  // A display name is chosen by its member, so promotion names the member's row, not the name.
  const copycat = await signupApp("board", "copycat@team.example");
  await assert.rejects(action(copycat, "cards.create", { title: "Before joining" }), /Join the board before adding a card\./);
  await action(copycat, "me.join", { display: "Member" });
  assert.equal((await action(lead, "cards.move", { id: card.id, status: "doing" })).changes, 1);
  assert.equal((await action(lead, "members.promote", { id: idOf("Member") })).changes, 1);
  assert.deepEqual((await action<Array<{ display: string; role: string }>>(lead, "members.list")).map((row) => [row.display, row.role]),
    [["Lead", "admin"], ["Member", "admin"], ["Developer", "member"], ["Member", "member"]]);
  await assert.rejects(action(lead, "members.promote", { id: idOf("Member") }), /and only one who is not an admin yet/);
  assert.equal((await action(member, "cards.delete", { id: card.id })).changes, 1);
  check("Team board", "Four users obey SQL role checks with each guard's sentence, an outsider cannot sign up, the owner seeds the first admin, and promotion names a member's row, so a second member who picks the same display name is not promoted with it.");

  await owner.call("signup_policy_set", { deploymentId: receipts.portfolio.id, mode: "allowlist", allowlist: ["potter@example.test"] });
  const potter = await signupApp("portfolio", "potter@example.test");
  const png = Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a8ioAAAAASUVORK5CYII="), (c) => c.charCodeAt(0));
  const transfer = await potter.upload(png);
  let file: { id: string; size: number };
  try { file = await potter.call("files_upload", { transferId: transfer.transferId, name: "pot.png", type: "image/png" }); }
  finally { await potter.cancel(transfer.transferId); }
  assert.equal(file.size, png.length);
  const downloaded = await potter.call<any>("files_read", { fileId: file.id });
  assert.deepEqual(await potter.download(downloaded), png);
  await action(potter, "pieces.create", { title: "Clay cup", body: "Wheel thrown", fileId: file.id, imageData: "data:image/png;base64," + Buffer.from(png).toString("base64") });
  const visitor = runtime("portfolio");
  const piece = (await action(visitor, "pieces.list"))[0];
  assert.equal(piece.title, "Clay cup");
  assert(piece.image_data.startsWith("data:image/png;base64,"));
  // A visitor gets no file ID; the potter reads hers from pieces.files and loads it with files.url's path.
  assert.equal("file_id" in piece, false);
  assert.deepEqual(await action(potter, "pieces.files"), [{ id: piece.id, file_id: file.id }]);
  await assert.rejects(action(visitor, "pieces.files"), /authentication-required/);
  await assert.rejects(visitor.call("files_read", { fileId: file.id }), /File not found/);
  assert.deepEqual(await visitor.call("files_list"), []);
  const poster = await potter.upload(png);
  let published: { id: string };
  try { published = await potter.call("files_upload", { transferId: poster.transferId, name: "poster.png", type: "image/png", visibility: "public" }); }
  finally { await potter.cancel(poster.transferId); }
  const listed = await visitor.call<Array<{ id: string; visibility: string; ownerId?: string }>>("files_list");
  assert.deepEqual(listed.map((item) => item.id), [published.id]);
  assert.equal(listed[0].visibility, "public");
  assert.equal(listed[0].ownerId, undefined);
  const publicDownload = await visitor.call<any>("files_read", { fileId: published.id });
  // download() releases the transfer when it finishes, and the Kernel refuses a second cancel.
  assert.deepEqual(await visitor.download(publicDownload), png);
  await assert.rejects(visitor.call("transfer_status", { transferId: publicDownload.transferId }), /Transfer not found or expired/);
  check("Public portfolio", "A private upload stays with the potter, and a visitor's piece list carries no file ID. A public upload can be listed and downloaded by a visitor.");

  const poll = runtime("poll");
  for (const choice of ["Tuesday", "Tuesday", "Sunday"]) assert.equal((await action(poll, "votes.cast", { choice })).changes, 1);
  await assert.rejects(action(poll, "votes.cast", { choice: "invalid" }), /Pick Tuesday, Thursday, Saturday or Sunday\./);
  const votes = await action(poll, "votes.results");
  assert.deepEqual(votes.map((row: any) => [row.choice, row.n]), [["Tuesday", 2], ["Sunday", 1]]);
  assert.equal((await action(poll, "votes.perhour"))[0].n, 3);
  check("Poll dashboard", "Anonymous votes feed grouped totals and an hourly report, and a choice outside the four days is refused with the guard's sentence.");

  await sql("feed", "INSERT INTO incidents(title,body,started_at) VALUES('Maintenance','Planned work',1700000000000)");
  const feed = runtime("feed");
  assert.equal((await action(feed, "incidents.recent"))[0].title, "Maintenance");
  // A signed route is its action's only door, so the runtime endpoint refuses it.
  await assert.rejects(action(feed, "pings.receive", { source: "monitor", payload: '{"status":"ok"}' }), /answers only on its HTTP route, POST \/hooks\/pings/);
  const hook = await fetch(url + "/hook", { method: "POST", headers: { host: receipts.feed.hostname, "content-type": "application/json" }, body: '{"status":"ok"}' });
  assert.equal(hook.status, 405);
  // A monitor posts with curl. The signature is HMAC-SHA256, keyed with the owner's secret,
  // of t, a nonce, the method, the path, the canonical query and the body.
  const curl = async (body: string, signature?: string) => {
    const args = ["curl", "-s", "-o", "-", "-w", "\n%{http_code}", "-X", "POST", `${url}/hooks/pings`,
      "-H", `Host: ${receipts.feed.hostname}`, "-H", "Content-Type: application/json", "-H", `X-Forwarded-For: ${freshAddress()}`, "--data-binary", body];
    if (signature) args.push("-H", `X-Signature: ${signature}`);
    const child = Bun.spawn(args, { stdout: "pipe", stderr: "inherit" });
    const output = await new Response(child.stdout).text();
    assert.equal(await child.exited, 0, "curl must run.");
    const [text, status] = [output.slice(0, output.lastIndexOf("\n")), Number(output.slice(output.lastIndexOf("\n") + 1))];
    return { status, body: JSON.parse(text) };
  };
  const ping = JSON.stringify({ source: "uptime", payload: '{"status":"ok"}' });
  const signature = await signRoute(pingsSecret, { method: "POST", path: "/hooks/pings", body: ping });
  assert.deepEqual(await curl(ping), { status: 401, body: { error: "invalid-signature" } });
  assert.deepEqual(await curl(ping, signature), { status: 200, body: { result: { changes: 1, lastInsertId: 1 } } });
  assert.deepEqual(await curl(ping, signature), { status: 409, body: { error: "replayed-signature" } });
  const incidents = await fetch(`${url}/api/incidents`, { headers: { host: receipts.feed.hostname, "x-forwarded-for": freshAddress() } });
  assert.equal(incidents.headers.get("content-type"), "application/json");
  assert.equal((await incidents.json()).result[0].title, "Maintenance");
  // A program reads the private badge with a key; a visitor gets nothing.
  const reader = await owner.call<{ key: string }>("api_keys_create", { deploymentId: receipts.feed.id, name: "Badge mirror", files: ["private"] });
  const byKey = await fetch(`${url}/_konae/files/${privateBadge.id}`, { headers: { host: receipts.feed.hostname, authorization: `Bearer ${reader.key}` } });
  assert.equal(byKey.status, 200);
  assert.equal(byKey.headers.get("content-type"), "image/png");
  assert.deepEqual(new Uint8Array(await byKey.arrayBuffer()), badgePng);
  assert.equal((await fetch(`${url}/_konae/files/${privateBadge.id}`, { headers: { host: receipts.feed.hostname } })).status, 404);
  let refused = 0;
  for (let i = 0; i < 35; i++) {
    try { await action(feed, "incidents.recent"); } catch (error) { assert.match(String(error), /rate-limited/); refused++; }
  }
  assert(refused > 0);
  check("API and webhook boundary", "JSON-RPC reads work and the signed action refuses JSON-RPC. curl's signed ping is stored once, the unsigned one and its replay are refused, GET /api/incidents returns rows, a key reads the private badge a visitor cannot, unknown POST paths return 405 and anonymous rate limits trip.");

  await sql("docs", "INSERT INTO pages(path,title,body) VALUES('/guide/install.html','Install the kiln','Install safely before firing')");
  assert.equal((await action(runtime("docs"), "pages.search", { q: "install" }))[0].title, "Install the kiln");
  assert.match(await (await getPage("docs", "/guide/install.html")).text(), /<title>Install/);
  const missing = await getPage("docs", "/missing-page");
  assert.equal(missing.status, 404);
  assert.doesNotMatch(await missing.text(), /<title>Home/);
  const pretty = await getPage("docs", "/guide/install");
  assert.equal(pretty.status, 200);
  assert.match(await pretty.text(), /<title>Install/);
  const sitemap = await getPage("docs", "/sitemap.xml");
  assert.equal(sitemap.headers.get("content-type"), "application/xml; charset=utf-8");
  assert.equal((await getPage("docs", "/robots.txt")).status, 200);
  check("Static docs", `konae build publishes the pages, pages/guide/ at /guide/ and public/; search works. Missing route status ${missing.status}; /guide/install serves the install page; sitemap type=${sitemap.headers.get("content-type")}.`);

  const retryRoot = path.join(scratch, "retry");
  await cp(path.join(import.meta.dir, "launch"), retryRoot, { recursive: true });
  const originalSchema = await readFile(path.join(retryRoot, "schema.sql"), "utf8");
  await writeFile(path.join(retryRoot, "schema.sql"), "CREATE VIEW unsupported AS SELECT 1;");
  const retryConfig = { url, project: { id: "product_fit_retry", name: "Retry", slug: "retry", manifest: { version: 1, visibility: "public" } } };
  const token = await owner.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish"], ttlSeconds: 60 });
  for (let i = 0; i < 9; i++) await assert.rejects(publishLocal(retryRoot, retryConfig, token.token), /schema/i);
  await writeFile(path.join(retryRoot, "schema.sql"), originalSchema);
  const originalActions = await readFile(path.join(retryRoot, "actions.json"), "utf8");
  const returningActions = JSON.parse(originalActions);
  returningActions.actions["waitlist.join"].statement += " RETURNING id";
  await writeFile(path.join(retryRoot, "actions.json"), JSON.stringify(returningActions));
  await assert.rejects(publishLocal(retryRoot, retryConfig, token.token), /cannot use RETURNING/);
  await writeFile(path.join(retryRoot, "actions.json"), originalActions);
  assert((await publishLocal(retryRoot, retryConfig, token.token)).url.startsWith("http://retry-"));
  check("Failed publish recovery", "Nine failed release plans free their transfers, and the repaired project publishes with the same credential.");

  const probe = runtime("probe");
  assert.equal((await action(probe, "items.add", { title: "Probe row" })).changes, 1);
  const optional = await action(probe, "items.optional");
  assert(optional.some((row: any) => row.title === "Probe row"));
  assert.deepEqual(await action(probe, "items.optional", { q: null }), optional);
  assert.equal((await action(probe, "items.optional", { q: "Probe" }))[0].title, "Probe row");
  check("Action edge cases", `RETURNING mutations fail release planning. Nullable search returned ${optional.length} rows with an omitted input; explicit null and filtered input also pass.`);

  // Each example's Action Contract, run against the Kernel that serves it. A
  // replacement backend would run the same checks. Every request comes from
  // its own address, so the anonymous rate limit does not slow the run.
  const sessionToken = async (name: string, email: string) => {
    const response = await fetch(`${url}/_konae/auth/login`, {
      method: "POST", headers: { host: receipts[name].hostname, "content-type": "application/json", "x-forwarded-for": freshAddress() },
      body: JSON.stringify({ email, password }),
    });
    assert.equal(response.status, 200, await response.clone().text());
    return response.headers.get("set-cookie")!.split(";")[0].split("=").slice(1).join("=");
  };
  await signupApp("probe", "conform@example.test");
  const tokens: Record<string, string> = {
    journal: await sessionToken("journal", "ana@example.test"),
    board: await sessionToken("board", "lead@team.example"),
    portfolio: await sessionToken("portfolio", "potter@example.test"),
    probe: await sessionToken("probe", "conform@example.test"),
  };
  const fromFreshAddress: typeof fetch = (input, init) => {
    const headers = new Headers(init?.headers); headers.set("x-forwarded-for", freshAddress());
    return fetch(input, { ...init, headers });
  };
  const conformance: Record<string, { passed: number; skipped: number }> = {};
  for (const name of Object.keys(receipts)) {
    const { contract } = generateActionContract({
      actionsSource: await readFile(path.join(import.meta.dir, name, "actions.json"), "utf8"),
      schemaSource: await readFile(path.join(import.meta.dir, name, "schema.sql"), "utf8"),
    });
    const report = await runConformance(contract, kernelTarget({
      url: receipts[name].url, token: tokens[name], fetch: fromFreshAddress,
      secrets: name === "feed" ? { pings_webhook: pingsSecret } : undefined,
    }), { inputs: { "items.jsonin": { ids: "[1, 2]" } } });
    assert.equal(report.failed, 0, `${name} does not conform to its Action Contract:\n${formatConformanceReport(report)}`);
    // cards.create guards an INSERT for members only. The session conform uses is a member,
    // so its second identical call inserts again and the guard cannot be reached. The team
    // board check above proves it with an account that has not joined.
    const unreachableGuards = name === "board" ? ["cards.create"] : [];
    const skips = report.results.filter((result) => result.outcome === "skip");
    assert.deepEqual(skips.map((result) => `${result.action}: ${result.check}`), unreachableGuards.map((action) => `${action}: refuses with its guard when no row changes`),
      `${name} skipped checks:\n${formatConformanceReport(report)}`);
    conformance[name] = { passed: report.passed, skipped: report.skipped };
  }
  // The CLI, from a contract file on disk, with no proxy header and no session.
  const contractDir = path.join(scratch, "contract-launch");
  await cp(path.join(import.meta.dir, "launch"), contractDir, { recursive: true });
  const cli = (args: string[]) => Bun.spawn(["bun", "run", path.join(repo, "scripts/konae.ts"), ...args], { cwd: repo, stdout: "pipe", stderr: "pipe" });
  const written = cli(["contract", "--dir", contractDir]);
  assert.equal(await written.exited, 0, await new Response(written.stderr).text());
  const conformCli = cli(["conform", "--url", receipts.launch.url, "--contract", path.join(contractDir, "action-contract.json")]);
  const [cliCode, cliOutput] = await Promise.all([conformCli.exited, new Response(conformCli.stdout).text()]);
  assert.equal(cliCode, 0, cliOutput + await new Response(conformCli.stderr).text());
  const cliTotals = cliOutput.trim().split("\n").at(-1)!;
  assert.match(cliTotals, /^\d+ passed, 0 failed, 0 skipped/);
  const total = Object.values(conformance).reduce((sum, item) => sum + item.passed, 0);
  check("Action Contracts", `All eight examples conform: ${total} checks pass, and the only skip is board's cards.create guard, which conform cannot reach with a member's session (${Object.entries(conformance).map(([name, item]) => `${name} ${item.passed}`).join(", ")}). The CLI run on launch reports "${cliTotals}"`);

  console.log(JSON.stringify({ examples: 8, checks: observations.length, observations }, null, 2));
} finally {
  kernel?.kill();
  if (kernel) await kernel.exited;
  await rm(scratch, { recursive: true, force: true });
}
