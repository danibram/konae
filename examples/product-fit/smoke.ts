import assert from "node:assert/strict";
import { cp, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../../src/mcp/client";
import { buildLocal, publishLocal } from "../../scripts/littlebox";

const repo = path.resolve(import.meta.dir, "../..");
const scratch = await mkdtemp(path.join(tmpdir(), "littlebox-product-fit-"));
const port = Number(process.env.LITTLEBOX_PRODUCT_FIT_PORT ?? 19410);
assert(Number.isInteger(port) && port >= 1024 && port <= 65535, "Choose a valid local port.");
const url = `http://127.0.0.1:${port}`;
const setupKey = crypto.randomUUID();
const password = crypto.randomUUID();
const observations: { scenario: string; result: string }[] = [];
let kernel: ReturnType<typeof Bun.spawn> | undefined;
let address = 0;
const freshAddress = () => `198.18.0.${++address}`;
const bytes = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));
function check(scenario: string, result: string) {
  observations.push({ scenario, result });
  console.log(`PASS ${scenario}. ${result}`);
}

async function tree(root: string, prefix = ""): Promise<Record<string, string>> {
  const files: Record<string, string> = {};
  for (const entry of await readdir(path.join(root, prefix), { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) Object.assign(files, await tree(root, relative));
    else files[relative] = await readFile(path.join(root, relative), "utf8");
  }
  return files;
}

try {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
  assert.equal(await build.exited, 0, "The Kernel must compile.");
  kernel = Bun.spawn([path.join(repo, "kernel-rs/target/debug/littlebox-kernel")], {
    cwd: repo,
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_") && !key.startsWith("VITE_"))),
      LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_PUBLIC_ORIGIN: url,
      LITTLEBOX_HOST_SUFFIX: "localhost", LITTLEBOX_DATA_DIR: path.join(scratch, "data"),
      LITTLEBOX_ADMIN_KEY: setupKey, LITTLEBOX_BUN_PATH: process.execPath, LITTLEBOX_SOURCE_ROOT: repo,
      LITTLEBOX_TRUSTED_PROXIES: "127.0.0.1",
    },
    stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    assert.equal(kernel.exitCode, null, "The isolated Kernel stopped before startup.");
    try { ready = (await fetch(`${url}/_littlebox/health`)).ok; } catch {}
    if (ready) break;
    await Bun.sleep(100);
  }
  assert(ready, "The isolated Kernel did not start.");
  const signup = await fetch(`${url}/_littlebox/owner/signup`, {
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
    const compiled = name === "docs" ? {
      artifact: { version: 1, entrypoint: "index.html", files: { ...await tree(path.join(root, "site")), "runtime.js": await readFile(path.join(repo, "src/mcp/runtime.js"), "utf8") } },
      files: { "schema.sql": await readFile(path.join(root, "schema.sql"), "utf8"), "actions.json": await readFile(path.join(root, "actions.json"), "utf8") },
    } : await buildLocal(root);
    const upload = await owner.upload(bytes({ project: { ...project, files: compiled.files }, visibility: project.manifest.visibility, artifact: compiled.artifact }), project.id);
    try {
      const plan = await owner.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
      receipts[name] = await owner.call("release_apply", { planId: plan.planId });
    } finally { await owner.cancel(upload.transferId); }
  }
  check("Publish eight examples", "All builds pass MCP release planning and apply on an empty Kernel.");

  function runtime(name: string, cookie?: string) {
    return new McpClient(`${url}/_littlebox/mcp`, { fetch: (input, init) => {
      const headers = new Headers(init?.headers); headers.set("host", receipts[name].hostname);
      if (cookie) headers.set("cookie", cookie);
      return fetch(input, { ...init, headers });
    } });
  }
  async function signupApp(name: string, email: string, expected = 200) {
    const response = await fetch(`${url}/_littlebox/auth/signup`, {
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
  await assert.rejects(action(launch, "waitlist.join", { email: "visitor@example.test", name: "Visitor", source: "smoke" }), /UNIQUE/);
  check("Anonymous waitlist", "A signed-out visitor joins once and the duplicate address fails.");

  const ana = await signupApp("journal", "ana@example.test");
  const bob = await signupApp("journal", "bob@example.test");
  assert.equal((await action(ana, "entries.create", { body: "Ana's private entry", mood: "calm" })).changes, 1);
  const entry = (await action(ana, "entries.list"))[0];
  assert.equal(entry.body, "Ana's private entry");
  assert.deepEqual(await action(bob, "entries.list"), []);
  assert.equal((await action(bob, "entries.delete", { id: entry.id })).changes, 0);
  await assert.rejects(action(runtime("journal"), "entries.list"), /authentication-required/);
  check("Private journal rows", "A second account cannot read or delete the first account's entry.");

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
  assert.equal((await action(dev, "cards.move", { id: card.id, status: "done" })).changes, 0);
  assert.equal((await action(dev, "cards.delete", { id: card.id })).changes, 0);
  assert.equal((await action(dev, "members.promote", { display: "Developer" })).changes, 0);
  assert.equal((await action(lead, "cards.move", { id: card.id, status: "doing" })).changes, 1);
  assert.equal((await action(lead, "members.promote", { display: "Member" })).changes, 1);
  assert.equal((await action(member, "cards.delete", { id: card.id })).changes, 1);
  check("Team board", "Three users obey SQL role checks, an outsider cannot sign up, and the owner seeds the first admin.");

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
  check("Public portfolio", "A private upload stays with the potter. A public upload can be listed and downloaded by a visitor.");

  const poll = runtime("poll");
  for (const choice of ["Tuesday", "Tuesday", "Sunday"]) assert.equal((await action(poll, "votes.cast", { choice })).changes, 1);
  assert.equal((await action(poll, "votes.cast", { choice: "invalid" })).changes, 0);
  const votes = await action(poll, "votes.results");
  assert.deepEqual(votes.map((row: any) => [row.choice, row.n]), [["Tuesday", 2], ["Sunday", 1]]);
  assert.equal((await action(poll, "votes.perhour"))[0].n, 3);
  check("Poll dashboard", "Anonymous votes feed grouped totals and an hourly report, with SQL checking allowed choices.");

  await sql("feed", "INSERT INTO incidents(title,body,started_at) VALUES('Maintenance','Planned work',1700000000000)");
  const feed = runtime("feed");
  assert.equal((await action(feed, "incidents.recent"))[0].title, "Maintenance");
  assert.equal((await action(feed, "pings.receive", { source: "monitor", payload: '{"status":"ok"}' })).changes, 1);
  const hook = await fetch(url + "/hook", { method: "POST", headers: { host: receipts.feed.hostname, "content-type": "application/json" }, body: '{"status":"ok"}' });
  assert.equal(hook.status, 405);
  let refused = 0;
  for (let i = 0; i < 35; i++) {
    try { await action(feed, "incidents.recent"); } catch (error) { assert.match(String(error), /rate-limited/); refused++; }
  }
  assert(refused > 0);
  check("API and webhook boundary", "JSON-RPC reads and writes work; conventional webhook POST returns 405 and anonymous rate limits trip.");

  await sql("docs", "INSERT INTO pages(path,title,body) VALUES('/guide/install.html','Install the kiln','Install safely before firing')");
  assert.equal((await action(runtime("docs"), "pages.search", { q: "install" }))[0].title, "Install the kiln");
  assert.match(await (await getPage("docs", "/guide/install.html")).text(), /<title>Install/);
  const missing = await getPage("docs", "/missing-page");
  const pretty = await getPage("docs", "/guide/install");
  const sitemap = await getPage("docs", "/sitemap.xml");
  check("Static docs", `Search and .html pages work. Missing route status ${missing.status}; pretty URL serves install=${/<title>Install/.test(await pretty.text())}; sitemap type=${sitemap.headers.get("content-type")}.`);

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
  console.log(JSON.stringify({ examples: 8, checks: observations.length, observations }, null, 2));
} finally {
  kernel?.kill();
  if (kernel) await kernel.exited;
  await rm(scratch, { recursive: true, force: true });
}
