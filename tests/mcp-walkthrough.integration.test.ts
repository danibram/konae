import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient, encodeBase64, type Transfer } from "../src/mcp/client";
import { signRoute } from "../src/lib/route-signature";
import { Recorder, callTool, cli, renderDocument, startKernel, type KernelProcess, type ToolAnswer } from "./mcp-walkthrough";

// Follows docs/mcp-walkthrough.md step by step on two Kernels and fails when
// the document does not show exactly the commands and answers this run got.
// LITTLEBOX_WALKTHROUGH_WRITE=1 rewrites the document's recorded blocks.
// LITTLEBOX_WALKTHROUGH_PORT sets the first of three ports: Kernel A, Kernel B
// and the Development Kernel.

const repo = path.resolve(import.meta.dir, "..");
const docPath = path.join(repo, "docs/mcp-walkthrough.md");
const base = Number(process.env.LITTLEBOX_WALKTHROUGH_PORT) || 19300 + Math.floor(Math.random() * 300) * 3;
const ports = { a: base, b: base + 1, dev: base + 2 };
const setupKey = "walkthrough-installation-key";
const setupKeyB = "walkthrough-second-installation-key";
const password = "walkthrough-agent-password";
const memberPassword = "walkthrough-member-password";
const secret = "walkthrough-orders-secret-0123";
const today = new Date().toISOString().slice(0, 10);

const APP_FILES: Record<string, string> = {
  "index.html": `<main>
  <h1>Orders</h1>
  <p><a href="/about">About this app</a></p>
  <button id="claim">Claim the oldest open order</button>
  <ul id="orders"></ul>
</main>
`,
  "pages/about.html": `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>About Orders</title><link rel="stylesheet" href="/styles.css"></head>
<body><main><h1>About</h1><p>Orders arrive from the shop's webhook. Members claim them.</p><p><a href="/">Back</a></p></main></body>
</html>
`,
  "styles.css": "body { font: 16px system-ui; max-width: 48rem; margin: 3rem auto; padding: 1rem; }\n",
  "app.js": `const list = document.querySelector("#orders");
const user = await miniapp.auth.session();
if (user.role === "anonymous") await miniapp.auth.signIn();
async function show() {
  const orders = await miniapp.actions.call("orders.list");
  list.replaceChildren(...orders.map((order) =>
    Object.assign(document.createElement("li"), { textContent: \`\${order.external_id}: \${order.total} (\${order.status})\` })));
}
document.querySelector("#claim").addEventListener("click", async () => {
  const [open] = (await miniapp.actions.call("orders.list")).filter((order) => order.status === "open");
  if (!open) return;
  try { await miniapp.actions.call("orders.claim", { order: open.id }); } catch (error) { alert(error.message); }
  await show();
});
await show();
`,
  "schema.sql": `CREATE TABLE IF NOT EXISTS orders(
  id INTEGER PRIMARY KEY,
  external_id TEXT NOT NULL UNIQUE,
  total REAL NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',
  claimed_by TEXT
);
`,
  "actions.json": JSON.stringify({
    version: 1,
    actions: {
      "orders.receive": {
        kind: "mutation", access: "anonymous",
        http: { method: "POST", path: "/hooks/orders", signature: { header: "X-Signature", secret: "orders_webhook" } },
        statement: "INSERT INTO orders (external_id, total) VALUES (?1, ?2) ON CONFLICT (external_id) DO NOTHING",
        bindings: [{ source: "input", key: "id", type: "string", maxLength: 64 }, { source: "input", key: "total", type: "number" }],
        signal: { name: "order.received", properties: ["id"] },
      },
      "orders.feed": {
        kind: "query", access: "anonymous",
        http: { method: "GET", path: "/api/orders", callers: ["key"] },
        statement: "SELECT id, external_id, total, status FROM orders WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT 100",
        bindings: [{ source: "input", key: "after", type: "number", nullable: true }],
      },
      "orders.list": {
        kind: "query", access: "authenticated",
        statement: "SELECT id, external_id, total, status FROM orders ORDER BY id LIMIT 50",
        bindings: [],
      },
      "orders.claim": {
        kind: "mutation", access: "authenticated",
        guard: "This order is already claimed.",
        statement: "UPDATE orders SET status = 'claimed', claimed_by = ?2 WHERE id = ?1 AND status = 'open'",
        bindings: [{ source: "input", key: "order", type: "number" }, { source: "actor", field: "id" }],
        signal: { name: "order.claimed", properties: ["order", "actor"] },
      },
    },
  }, null, 2) + "\n",
};

const NOTES = {
  id: "app_walkthrough_notes", name: "Notes", slug: "notes", updatedAt: 1762430000000,
  files: {
    "index.html": "<main id=app></main>",
    "app.js": "document.querySelector('#app').textContent = 'Notes';\n",
    "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);\n",
    "actions.json": "{\"version\":1,\"actions\":{}}\n",
  },
  directories: [],
  databaseModel: { source: "", acceptedSource: "", revision: "r1" },
  manifest: { version: 1, visibility: "private", runtime: { database: { mode: "declared-actions", actions: [] }, auth: { mode: "optional" }, files: { access: "deployment" } } },
};

const r = new Recorder();
let root = "";
let a: KernelProcess;
let b: KernelProcess | undefined;
/** What later steps need from earlier ones. */
const s: Record<string, any> = {};
const app = () => path.join(root, "orders");
const shownPort: Record<number, number> = {};

function shownCommand(args: string[], env: Record<string, string> = {}) {
  return [...Object.entries(env).map(([key, value]) => `${key}=${value}`), "bun run littlebox --", ...args].join(" ");
}

async function run(args: string[], env: Record<string, string> = {}, shownEnv = env) {
  const result = await cli(args, env);
  r.add("sh", shownCommand(args, shownEnv));
  r.add("text", result.output || "(no output)");
  return result;
}

function answerBlock(answer: ToolAnswer, pick?: (value: any) => unknown) {
  return answer.result !== undefined ? { result: pick ? pick(answer.result) : answer.result } : answer;
}

async function tool(name: string, args: object = {}, options: { token?: string; kernel?: KernelProcess; pick?: (value: any) => unknown } = {}): Promise<ToolAnswer> {
  const kernel = options.kernel ?? a;
  const answer = await callTool(`${kernel.url}/mcp`, { authorization: `Bearer ${options.token ?? s.token}` }, name, args);
  r.json({ name, arguments: args });
  r.json(answerBlock(answer, options.pick));
  return answer;
}

async function runtimeTool(hostname: string, cookie: string | undefined, name: string, args: object = {}, kernel = a) {
  const answer = await callTool(`${kernel.url}/_littlebox/mcp`, { host: hostname, ...(cookie ? { cookie } : {}) }, name, args);
  r.json({ name, arguments: args });
  r.json(answerBlock(answer));
  return answer;
}

/** A request to an app host, shown as the curl an agent would send to it. */
async function http(method: string, kernel: { url: string; port: number }, route: string, init: { host?: string; headers?: Record<string, string>; shownHeaders?: Record<string, string>; body?: string } = {}) {
  const response = await fetch(`${kernel.url}${route}`, { method, headers: { ...(init.host ? { host: init.host } : {}), ...init.headers }, body: init.body, redirect: "manual" });
  const shownUrl = `http://${init.host ?? "localhost"}:${shownPort[kernel.port]}${route}`;
  const headers = Object.entries(init.shownHeaders ?? init.headers ?? {}).map(([key, value]) => ` -H '${key}: ${value}'`).join("");
  r.add("sh", `curl -i${method === "GET" ? "" : ` -X ${method}`} '${shownUrl}'${headers}${init.body ? ` --data '${init.body}'` : ""}`);
  const type = response.headers.get("content-type") ?? "";
  const text = await response.text();
  const lines = [`HTTP ${response.status}`];
  for (const header of ["location", "content-type", "retry-after"]) if (response.headers.get(header)) lines.push(`${header}: ${response.headers.get(header)}`);
  if (type.includes("json")) lines.push("", text);
  r.add("text", lines.join("\n"));
  return { response, text };
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", path.join(repo, "kernel-rs/Cargo.toml")], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "littlebox-walkthrough-"));
  await mkdir(app());
  a = await startKernel(ports.a, path.join(root, "kernel-a"), setupKey);
  Object.assign(shownPort, { [ports.a]: 4173, [ports.b]: 4174, [ports.dev]: 8796 });
  r.name(root, "/tmp");
  for (const [port, shown] of Object.entries(shownPort)) {
    r.name(`http://127.0.0.1:${port}`, `http://localhost:${shown}`);
    r.name(`127.0.0.1:${port}`, `localhost:${shown}`);
    r.name(`localhost:${port}`, `localhost:${shown}`);
  }
  r.name(today, "<today>");
}, 300_000);

afterAll(async () => {
  for (const kernel of [a, b]) if (kernel) { kernel.child.kill(); await kernel.child.exited; }
  if (root) await rm(root, { recursive: true, force: true });
});

describe("the MCP walkthrough", () => {
  test("1. an Agent Credential from an invitation code", async () => {
    r.step("operator");
    const first = await cli(["login", "--dir", path.join(root, "operator"), "--url", a.url], { LITTLEBOX_EMAIL: "operator@walkthrough.test", LITTLEBOX_PASSWORD: password, LITTLEBOX_SETUP_KEY: setupKey });
    expect(first.code).toBe(0);
    const issued = Bun.spawnSync([path.join(repo, "kernel-rs/target/debug/littlebox-kernel"), "creator-code-create", "--note", "Walkthrough agent"], { env: { PATH: process.env.PATH ?? "", LITTLEBOX_DATA_DIR: a.data } });
    const printed = issued.stdout.toString().trim();
    s.code = printed.match(/shown once: (\S+)/)![1];
    r.name(s.code, "<invitation code>");
    r.name(printed.match(/ID: (\S+)/)![1], "<code id>");
    r.name(printed.match(/Expires: (.+)/)![1], "<expiry>");
    r.add("sh", "littlebox-kernel creator-code-create --note 'Walkthrough agent'");
    r.add("text", printed);

    r.step("login");
    const login = await run(["login", "--dir", app(), "--url", a.url, "--manage-users", "--export-data", "--manage-lifecycle"], { LITTLEBOX_EMAIL: "agent@walkthrough.test", LITTLEBOX_PASSWORD: password, LITTLEBOX_INVITATION_CODE: s.code });
    expect(login.code).toBe(0);
    s.token = JSON.parse(await readFile(path.join(app(), ".littlebox/credentials.json"), "utf8")).token;

    r.step("session");
    const session = await tool("session_inspect");
    r.name((session.result as any).ownerId, "<owner id>");
    expect((session.result as any).scopes).toEqual(["read", "write", "publish", "manage_users", "export_data", "manage_lifecycle"]);
    expect((session.result as any).can).toEqual({
      read: true,
      write: true,
      publish: true,
      manage_users: true,
      export_data: true,
      manage_lifecycle: true,
      createTokens: false,
    });
  }, 60_000);

  test("2. platform_describe and platform_guide", async () => {
    r.step("describe");
    const described = await tool("platform_describe");
    expect((described.result as any).guideTopics).toEqual(["rules", "pieces", "credentials", "development", "actions", "sdk", "publishing", "users", "portability", "refusals"]);
    expect((described.result as any).yardstick).toContain("take it to another machine");
    const rules = await callTool(`${a.url}/mcp`, { authorization: `Bearer ${s.token}` }, "platform_guide", { topic: "rules" });
    expect((rules.result as any).readBack.map.release_apply).toBe("project_inspect");
    expect((rules.result as any).firstRun.mcp).toHaveLength(3);
    r.step("guide");
    await tool("platform_guide", { topic: "credentials" });
  });

  test("3. an OAuth credential for a throwaway client", async () => {
    r.step("oauth-register");
    const registration = await http("POST", a, "/oauth/register", { headers: { "content-type": "application/json" }, body: JSON.stringify({ client_name: "Walkthrough agent", redirect_uris: ["http://127.0.0.1:9999/callback"] }) });
    const client = JSON.parse(registration.text);
    r.name(client.client_id, "<client id>");
    const verifier = "walkthrough-verifier-0123456789-0123456789-0123456789";
    const challenge = Buffer.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier)))).toString("base64url");
    r.name(challenge, "<code challenge>");
    const args = { client_id: client.client_id, redirect_uri: "http://127.0.0.1:9999/callback", response_type: "code", code_challenge_method: "S256", code_challenge: challenge, scope: "read write publish manage_users", resource: `${a.url}/mcp`, state: "walkthrough" };

    r.step("oauth-authorize");
    await http("GET", a, `/oauth/authorize?${new URLSearchParams(args)}`);
    const signIn = await fetch(`${a.url}/_littlebox/owner/login`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email: "agent@walkthrough.test", password }) });
    const cookie = signIn.headers.get("set-cookie")!.split(";")[0];
    r.name(cookie.split("=")[1], "<owner session>");
    const page = await (await fetch(`${a.url}/oauth/authorize?${new URLSearchParams(args)}`, { headers: { cookie } })).text();
    const csrf = page.match(/name=csrf value="([^"]+)"/)![1];
    r.name(csrf, "<csrf>");
    r.add("text", page.match(/<h1>.*?<\/p><p>.*?<\/p><p>.*?<\/p>/)![0].replace(/<\/(h1|p)>/g, "\n").replace(/<[^>]+>/g, "").replace(/ +\n/g, "\n").trim());
    const approved = await fetch(`${a.url}/oauth/authorize`, { method: "POST", redirect: "manual", headers: { cookie, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ ...args, csrf }) });
    const back = new URL(approved.headers.get("location")!);
    r.name(back.searchParams.get("code"), "<authorization code>");
    r.add("sh", "curl -i -X POST 'http://localhost:4173/oauth/authorize' -H 'cookie: littlebox_owner_session=<owner session>' --data '<the same parameters>&csrf=<csrf>'");
    r.add("text", `HTTP ${approved.status}\nlocation: ${approved.headers.get("location")}`);

    r.step("oauth-token");
    const exchanged = await http("POST", a, "/oauth/token", { headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ grant_type: "authorization_code", client_id: client.client_id, redirect_uri: args.redirect_uri, code: back.searchParams.get("code")!, resource: args.resource, code_verifier: verifier }).toString() });
    s.oauth = JSON.parse(exchanged.text).access_token;
    const session = await tool("session_inspect", {}, { token: s.oauth });
    expect((session.result as any).scopes).toEqual(["read", "write", "publish", "manage_users"]);

    r.step("oauth-scope");
    const refused = await http("GET", a, `/oauth/authorize?${new URLSearchParams({ ...args, scope: "read export_data" })}`, { headers: { cookie }, shownHeaders: { cookie: "littlebox_owner_session=<owner session>" } });
    expect(JSON.parse(refused.text).error_description).toBe("Invalid scope. Request one or more of read, write, publish and manage_users, separated by spaces. OAuth does not offer export_data or manage_lifecycle; run littlebox login with --export-data or --manage-lifecycle for a token that holds them.");
  }, 60_000);

  test("4. init", async () => {
    r.step("init");
    const config = async () => JSON.parse(await readFile(path.join(app(), "littlebox.json"), "utf8"));
    const init = await cli(["init", "--dir", app(), "--name", "Orders", "--url", a.url]);
    expect(init.code).toBe(0);
    s.projectId = (await config()).project.id;
    r.name(s.projectId, "<project id>");
    r.add("sh", shownCommand(["init", "--dir", app(), "--name", "Orders", "--url", a.url]));
    r.add("text", init.output);
    r.add("sh", "cat /tmp/orders/littlebox.json");
    r.add("json", JSON.stringify(await config(), null, 2));
    r.add("sh", "ls /tmp/orders");
    r.add("text", (await readdir(app())).filter((name) => !name.startsWith(".")).sort().join("\n"));
  }, 60_000);

  test("5. dev", async () => {
    r.step("dev");
    const child = Bun.spawn([process.execPath, path.join(repo, "scripts/littlebox.ts"), "dev", "--dir", app(), "--port", String(ports.dev)], {
      cwd: repo, env: Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_"))) as Record<string, string>, stdout: "pipe", stderr: "ignore",
    });
    try {
      const reader = child.stdout.getReader();
      let printed = "";
      const deadline = Date.now() + 240_000;
      while (!/http:\/\/\S+\.localhost:\d+/.test(printed) && Date.now() < deadline) {
        const { value, done } = await reader.read();
        if (done) break;
        printed += new TextDecoder().decode(value);
      }
      const url = printed.match(/http:\/\/(\S+\.localhost):\d+/)!;
      r.name(url[1].split(".")[0], "orders-<dev suffix>");
      r.add("sh", shownCommand(["dev", "--dir", app(), "--port", "8796"]));
      r.add("text", printed.trim().split("\n").filter((line) => line.includes(".localhost:")).join("\n"));
      const { response } = await http("GET", { url: `http://127.0.0.1:${ports.dev}`, port: ports.dev }, "/", { host: url[1] });
      expect(response.status).toBe(200);
      reader.releaseLock();
    } finally {
      child.kill();
      await child.exited;
    }
  }, 300_000);

  test("6. the app's files, and its Action Contract", async () => {
    r.step("files");
    for (const [name, text] of Object.entries(APP_FILES)) {
      await mkdir(path.dirname(path.join(app(), name)), { recursive: true });
      await writeFile(path.join(app(), name), text);
      r.add("sh", `cat /tmp/orders/${name}`);
      r.add(path.extname(name).slice(1), text);
    }
    r.step("contract");
    expect((await run(["contract", "--dir", app()])).code).toBe(0);
  });

  test("7. a signing secret, then publish with the policy littlebox.json names", async () => {
    r.step("publish-refused");
    const refused = await run(["publish", "--dir", app()]);
    expect(refused.code).toBe(1);
    expect(refused.output).toBe("Action orders.receive signs with the secret orders_webhook, which is not set for this project. Set it with secret_set before publishing.");

    r.step("secret");
    await tool("secret_set", { projectId: s.projectId, name: "orders_webhook", value: secret });

    r.step("publish");
    expect((await run(["publish", "--dir", app()])).code).toBe(0);
    const receipt = JSON.parse(await readFile(path.join(app(), ".littlebox/deployment.json"), "utf8"));
    Object.assign(s, { deploymentId: receipt.id, hostname: receipt.hostname });
    r.name(receipt.id, "<deployment id>");
    r.name(receipt.hostname.split(".")[0], "orders-<suffix>");
    const policy = await tool("signup_policy_read", { deploymentId: s.deploymentId });
    expect((policy.result as any).mode).toBe("invitation");
    await http("GET", a, "/about", { host: s.hostname });
  }, 120_000);

  test("8. invite a member through MCP", async () => {
    r.step("invite-refused");
    const publisher = await run(["login", "--dir", path.join(root, "publisher"), "--url", a.url], { LITTLEBOX_EMAIL: "agent@walkthrough.test", LITTLEBOX_PASSWORD: password });
    expect(publisher.code).toBe(0);
    const publishOnly = JSON.parse(await readFile(path.join(root, "publisher/.littlebox/credentials.json"), "utf8")).token;
    const refused = await tool("app_invitations_create", { deploymentId: s.deploymentId, emails: ["member@walkthrough.test"], idempotencyKey: "walkthrough-invite-1" }, { token: publishOnly });
    expect(refused.error).toBe("Unknown tool or insufficient permission. app_invitations_create needs a credential with manage_users. Run littlebox login with --manage-users, or use a token from token_create that includes it.");

    r.step("invite");
    await tool("mail_status", {}, { pick: (value) => ({ transport: value.transport, sending: value.sending }) });
    const invited = await tool("app_invitations_create", { deploymentId: s.deploymentId, emails: ["member@walkthrough.test"], idempotencyKey: "walkthrough-invite-1" });
    r.name((invited.result as any).results[0].invitationId, "<invitation id>");

    r.step("accept");
    const mail = (await readFile(path.join(a.data, "mailbox.jsonl"), "utf8")).trim().split("\n").map((line) => JSON.parse(line));
    const message = mail.findLast((entry: any) => entry.to === "member@walkthrough.test");
    const token = new URL(message.link).hash.replace("#invitation=", "");
    r.name(token, "<invitation token>");
    r.name(a.data, "/tmp/kernel-a");
    r.add("sh", "tail -n 1 /tmp/kernel-a/mailbox.jsonl");
    r.add("json", JSON.stringify(message));
    await http("POST", a, "/_littlebox/auth/invitation", { host: s.hostname, headers: { "content-type": "application/json" }, body: JSON.stringify({ token }) });
    const accepted = await http("POST", a, "/_littlebox/auth/accept", { host: s.hostname, headers: { "content-type": "application/json" }, body: JSON.stringify({ token, password: memberPassword }) });
    expect(accepted.response.status).toBe(200);
    s.cookie = accepted.response.headers.get("set-cookie")!.split(";")[0];
    s.session = s.cookie.split("=")[1];
    r.name(s.session, "<app session>");
    r.name(JSON.parse(accepted.text).result.user.userId, "<member id>");
    r.add("text", `set-cookie: ${s.cookie}; ...`);
  }, 60_000);

  test("9. the member's calls: the second page, a signed route, a guarded mutation and a private file", async () => {
    r.step("runtime");
    await runtimeTool(s.hostname, s.cookie, "auth_session");
    await runtimeTool(s.hostname, s.cookie, "actions_invoke", { name: "orders.list" });
    const shownCookie = { cookie: "littlebox_app_session=<app session>" };
    expect((await http("GET", a, "/about", { host: s.hostname, headers: { cookie: s.cookie }, shownHeaders: shownCookie })).response.status).toBe(200);
    expect((await http("GET", a, "/missing", { host: s.hostname, headers: { cookie: s.cookie }, shownHeaders: shownCookie })).response.status).toBe(404);

    r.step("signed-route");
    const body = JSON.stringify({ id: "A-1", total: 12 });
    const signature = await signRoute(secret, { method: "POST", path: "/hooks/orders", body }, Math.floor(Date.now() / 1000), "walkthrough-nonce-1");
    r.name(signature.match(/v1=([0-9a-f]+)/)![1], "<hmac>");
    expect((await http("POST", a, "/hooks/orders", { host: s.hostname, headers: { "content-type": "application/json", "x-signature": signature }, body })).response.status).toBe(200);
    expect((await http("POST", a, "/hooks/orders", { host: s.hostname, headers: { "content-type": "application/json", "x-signature": signature }, body })).response.status).toBe(409);
    expect((await http("POST", a, "/hooks/orders", { host: s.hostname, headers: { "content-type": "application/json" }, body })).response.status).toBe(401);

    r.step("guard");
    await runtimeTool(s.hostname, s.cookie, "actions_invoke", { name: "orders.claim", input: { order: 1 } });
    const again = await runtimeTool(s.hostname, s.cookie, "actions_invoke", { name: "orders.claim", input: { order: 1 } });
    expect(again.refused).toBe("This order is already claimed.");

    r.step("file");
    const bytes = new TextEncoder().encode("Invoice A-1: 12.00\n");
    const sha256 = new Bun.CryptoHasher("sha256").update(bytes).digest("hex");
    const begun = await runtimeTool(s.hostname, s.cookie, "transfer_begin", { size: bytes.length, sha256 });
    const transferId = (begun.result as any).transferId;
    r.name(transferId, "<transfer id>");
    await runtimeTool(s.hostname, s.cookie, "transfer_write", { transferId, offset: 0, data: encodeBase64(bytes) });
    await runtimeTool(s.hostname, s.cookie, "transfer_complete", { transferId });
    const stored = await runtimeTool(s.hostname, s.cookie, "files_upload", { transferId, name: "invoice-a1.txt", type: "text/plain" });
    s.fileId = (stored.result as any).id;
    r.name(s.fileId, "<file id>");
    expect((await http("GET", a, `/_littlebox/files/${s.fileId}`, { host: s.hostname, headers: { cookie: s.cookie }, shownHeaders: shownCookie })).response.status).toBe(200);
    expect((await http("GET", a, `/_littlebox/files/${s.fileId}`, { host: s.hostname })).response.status).toBe(401);
  }, 60_000);

  test("10. an API key for the feed route", async () => {
    r.step("key");
    const created = await tool("api_keys_create", { deploymentId: s.deploymentId, name: "Accounting export", actions: ["orders.feed"], files: [] });
    s.key = (created.result as any).key;
    r.name((created.result as any).keyId, "<key id>");
    expect((await http("GET", a, "/api/orders", { host: s.hostname, headers: { authorization: `Bearer ${s.key}` }, shownHeaders: { authorization: "Bearer <api key>" } })).response.status).toBe(200);
    expect((await http("GET", a, "/api/orders", { host: s.hostname })).response.status).toBe(401);
    expect((await http("GET", a, `/_littlebox/files/${s.fileId}`, { host: s.hostname, headers: { authorization: `Bearer ${s.key}` }, shownHeaders: { authorization: "Bearer <api key>" } })).response.status).toBe(404);
  }, 60_000);

  test("11. conform", async () => {
    r.step("conform");
    const result = await cli(["conform", "--url", `http://${s.hostname}:${ports.a}`, "--contract", path.join(app(), "action-contract.json"), "--token", s.session, "--key", s.key, "--secret", `orders_webhook=${secret}`]);
    r.add("sh", shownCommand(["conform", "--url", `http://${s.hostname}:${ports.a}`, "--contract", "/tmp/orders/action-contract.json", "--token", "<app session>", "--key", "<api key>", "--secret", "orders_webhook=<secret>"]));
    r.add("text", result.output);
    expect(result.code).toBe(0);
  }, 120_000);

  test("12. Usage and Signals", async () => {
    r.step("usage");
    await tool("usage_read", { deploymentId: s.deploymentId, from: today, to: today }, { pick: (value) => ({ from: value.from, to: value.to, totals: value.totals }) });
    r.step("signals");
    const signals = await tool("signals_read", { deploymentId: s.deploymentId });
    // conform sends one generated order; name it so the document is the same on every run.
    const generated = (signals.result as any).signals.find((signal: any) => signal.name === "order.received" && signal.properties.id !== "A-1");
    r.name(generated?.properties.id, "<conform order>");
  }, 60_000);

  test("13. export the package, import it on a second Kernel", async () => {
    r.step("export-refused");
    const refused = await tool("package_export", { deploymentId: s.deploymentId }, { token: s.oauth });
    expect(refused.error).toBe("Unknown tool or insufficient permission. package_export needs a credential with export_data. Run littlebox login with --export-data, or use a token from token_create that includes it.");

    r.step("export");
    const exported = await tool("package_export", { deploymentId: s.deploymentId });
    const result = exported.result as { manifest: any; downloads: Record<string, Transfer> };
    r.name(result.manifest.packageId, "<package id>");
    r.name(result.manifest.artifactVersion, "<artifact version>");
    for (const [name, transfer] of Object.entries(result.downloads)) r.name(transfer.transferId, `<${name} download>`);
    const first = await callTool(`${a.url}/mcp`, { authorization: `Bearer ${s.token}` }, "transfer_read", { transferId: result.downloads.identity.transferId, offset: 0 });
    r.json({ name: "transfer_read", arguments: { transferId: result.downloads.identity.transferId, offset: 0 } });
    r.json({ result: { ...(first.result as object), data: "<base64 of the first 65536 bytes>" } });
    const source = new McpClient(`${a.url}/mcp`, { token: s.token });
    const components: Record<string, Uint8Array> = {};
    for (const [name, transfer] of Object.entries(result.downloads)) components[name] = await source.download(transfer);

    r.step("second-kernel");
    b = await startKernel(ports.b, path.join(root, "kernel-b"), setupKeyB);
    const destination = path.join(root, "orders-b");
    expect((await run(["login", "--dir", destination, "--url", b.url, "--manage-users"], { LITTLEBOX_EMAIL: "agent@walkthrough.test", LITTLEBOX_PASSWORD: password, LITTLEBOX_SETUP_KEY: setupKeyB })).code).toBe(0);
    s.tokenB = JSON.parse(await readFile(path.join(destination, ".littlebox/credentials.json"), "utf8")).token;
    const manifest = { version: 1, visibility: "private" };
    await tool("project_create", { projectId: s.projectId, name: "Orders", slug: "orders", manifest }, { token: s.tokenB, kernel: b, pick: (value) => ({ id: value.id, name: value.name, slug: value.slug, sourceAuthority: value.sourceAuthority }) });
    await tool("secret_set", { projectId: s.projectId, name: "orders_webhook", value: secret }, { token: s.tokenB, kernel: b });

    r.step("import-upload");
    const client = new McpClient(`${b.url}/mcp`, { token: s.tokenB });
    const uploads: Record<string, string> = {};
    for (const [name, bytes] of Object.entries(components)) {
      if (name !== "database") {
        uploads[name] = (await client.upload(bytes, s.projectId, name)).transferId;
        r.name(uploads[name], `<${name} upload>`);
        continue;
      }
      const sha256 = new Bun.CryptoHasher("sha256").update(bytes).digest("hex");
      const begun = await tool("transfer_begin", { size: bytes.length, sha256, projectId: s.projectId, component: name }, { token: s.tokenB, kernel: b });
      uploads[name] = (begun.result as any).transferId;
      r.name(uploads[name], `<${name} upload>`);
      for (let offset = 0; offset < bytes.length; offset += 65536) {
        const written = await client.call<object>("transfer_write", { transferId: uploads[name], offset, data: encodeBase64(bytes.subarray(offset, offset + 65536)) });
        if (offset === 0) {
          r.json({ name: "transfer_write", arguments: { transferId: uploads[name], offset: 0, data: "<base64 of up to 65536 bytes from offset 0>" } });
          r.json({ result: written });
        }
      }
      await tool("transfer_complete", { transferId: uploads[name] }, { token: s.tokenB, kernel: b });
    }
    expect(Object.keys(uploads).sort()).toEqual(["database", "files", "identity", "usage"]);

    r.step("import-release");
    expect((await run(["build", "--dir", app()])).code).toBe(0);
    const artifact = JSON.parse(await readFile(path.join(app(), ".littlebox/artifact.json"), "utf8"));
    const payload = { project: { id: s.projectId, name: "Orders", slug: "orders", manifest, files: APP_FILES }, visibility: "private", artifact, initialPackage: { manifest: result.manifest, components: uploads } };
    r.add("json", JSON.stringify({ ...payload, project: { ...payload.project, files: "<path: text of every source file>" }, artifact: "<the JSON in .littlebox/artifact.json>", initialPackage: { manifest: "<the manifest package_export returned>", components: uploads } }, null, 2));
    const release = await client.upload(new TextEncoder().encode(JSON.stringify(payload)), s.projectId);
    r.name(release.transferId, "<release upload>");
    const plan = await tool("release_plan", { transferId: release.transferId }, { token: s.tokenB, kernel: b });
    r.name((plan.result as any).planId, "<plan id>");
    const applied = await tool("release_apply", { planId: (plan.result as any).planId }, { token: s.tokenB, kernel: b, pick: (value) => ({ id: value.id, hostname: value.hostname, url: value.url, signupPolicy: value.signupPolicy }) });
    Object.assign(s, { deploymentB: (applied.result as any).id, hostnameB: (applied.result as any).hostname });
    r.name(s.deploymentB, "<destination deployment id>");
    r.name(s.hostnameB.split(".")[0], "orders-<destination suffix>");
    for (const id of [...Object.values(uploads), release.transferId]) await client.cancel(id).catch(() => {});

    r.step("import-check");
    const signIn = await http("POST", b, "/_littlebox/auth/login", { host: s.hostnameB, headers: { "content-type": "application/json" }, body: JSON.stringify({ email: "member@walkthrough.test", password: memberPassword }) });
    expect(signIn.response.status).toBe(200);
    const cookie = signIn.response.headers.get("set-cookie")!.split(";")[0];
    r.name(cookie.split("=")[1], "<destination app session>");
    const listed = await runtimeTool(s.hostnameB, cookie, "actions_invoke", { name: "orders.list" }, b);
    expect((listed.result as any[]).map((row) => row.external_id)).toContain("A-1");
    const keys = await tool("api_keys_list", { deploymentId: s.deploymentB }, { token: s.tokenB, kernel: b });
    expect((keys.result as any).keys[0].revokedReason).toBe("import");
  }, 180_000);

  test("14. Eject a Managed Project and publish from its folder", async () => {
    r.step("managed");
    await tool("project_create", { projectId: NOTES.id, name: "Notes", sourceAuthority: "managed" }, { pick: (value) => ({ id: value.id, name: value.name, slug: value.slug, sourceAuthority: value.sourceAuthority }) });
    r.add("json", JSON.stringify(NOTES, null, 2));
    const client = new McpClient(`${a.url}/mcp`, { token: s.token });
    const upload = await client.upload(new TextEncoder().encode(JSON.stringify(NOTES)), NOTES.id);
    r.name(upload.transferId, "<sources upload>");
    const saved = await tool("project_sources_save", { projectId: NOTES.id, transferId: upload.transferId, expectedRevision: 0 });
    expect(saved.result).toEqual({ revision: 1 });
    await client.cancel(upload.transferId).catch(() => {});

    r.step("eject");
    const folder = path.join(root, "notes");
    const ejected = await run(["eject", "--dir", folder, "--project", NOTES.id, "--url", a.url], { LITTLEBOX_MCP_TOKEN: s.token }, { LITTLEBOX_MCP_TOKEN: "<token>" });
    expect(ejected.code).toBe(0);
    r.add("sh", "ls /tmp/notes");
    r.add("text", (await readdir(folder)).filter((name) => !name.startsWith(".")).sort().join("\n"));

    r.step("eject-publish");
    expect((await run(["publish", "--dir", folder], { LITTLEBOX_MCP_TOKEN: s.token }, { LITTLEBOX_MCP_TOKEN: "<token>" })).code).toBe(0);
    const receipt = JSON.parse(await readFile(path.join(folder, ".littlebox/deployment.json"), "utf8"));
    r.name(receipt.hostname.split(".")[0], "notes-<suffix>");
    const change = await new McpClient(`${a.url}/mcp`, { token: s.token }).upload(new TextEncoder().encode(JSON.stringify({ ...NOTES, updatedAt: NOTES.updatedAt + 1 })), NOTES.id);
    r.name(change.transferId, "<another sources upload>");
    const after = await tool("project_sources_save", { projectId: NOTES.id, transferId: change.transferId, expectedRevision: 1 });
    await new McpClient(`${a.url}/mcp`, { token: s.token }).cancel(change.transferId).catch(() => {});
    expect(after.refused).toStartWith("This project was ejected");
  }, 120_000);

  test("15. begin and cancel a migration", async () => {
    r.step("migration-begin");
    const begun = await tool("migration_begin", { deploymentId: s.deploymentId }, { pick: (value) => ({ operationId: value.operationId, packageId: value.packageId, phase: value.phase, paused: value.paused, downloads: Object.keys(value.downloads), next: value.next }) });
    const operationId = (begun.result as any).operationId;
    r.name(operationId, "<operation id>");
    r.name((begun.result as any).packageId, "<migration package id>");
    expect((await http("POST", a, "/hooks/orders", { host: s.hostname, headers: { "content-type": "application/json" }, body: "{}" })).response.status).toBe(503);

    r.step("migration-cancel");
    await tool("migration_cancel", { operationId }, { pick: (value) => ({ operationId: value.operationId, effects: value.effects }) });
    const status = await tool("migration_status", { deploymentId: s.deploymentId }, { pick: (value) => ({ phase: value.phase, paused: value.paused, operationId: value.operationId }) });
    expect((status.result as any).paused).toBe(false);
    const client = new McpClient(`${a.url}/mcp`, { token: s.token });
    for (const transfer of Object.values((begun.result as any).downloads) as Transfer[]) await client.cancel(transfer.transferId).catch(() => {});
    const body = JSON.stringify({ id: "A-2", total: 30 });
    const signature = await signRoute(secret, { method: "POST", path: "/hooks/orders", body }, Math.floor(Date.now() / 1000), "walkthrough-nonce-2");
    r.name(signature.match(/v1=([0-9a-f]+)/)![1], "<hmac 2>");
    expect((await http("POST", a, "/hooks/orders", { host: s.hostname, headers: { "content-type": "application/json", "x-signature": signature }, body })).response.status).toBe(200);
  }, 120_000);

  test("docs/mcp-walkthrough.md shows what this run recorded", async () => {
    const document = await readFile(docPath, "utf8");
    const rendered = renderDocument(document, r);
    if (process.env.LITTLEBOX_WALKTHROUGH_WRITE) await writeFile(docPath, rendered);
    else expect(rendered, "Run with LITTLEBOX_WALKTHROUGH_WRITE=1 to record the new answers, then review the diff.").toBe(document);
  });
});
