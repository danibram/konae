import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";

// Delivery C of docs/plans/platform-launch.md: creator admission with
// invitation codes, against a Kernel that requires verification and writes
// its mail to a development mailbox. The operator commands run as the real
// executable against the live data directory.

const binary = "kernel-rs/target/debug/littlebox-kernel";
const setupKey = "admission-integration-setup-secret";
const password = "admission-password-2026";
let root: string;
let url: string;
let kernel: ReturnType<typeof Bun.spawn>;
let nextClient = 0;
const freshClient = () => { nextClient++; return `198.19.${Math.floor(nextClient / 250)}.${nextClient % 250 + 1}`; };

function operatorEnv() {
  return { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_"))), LITTLEBOX_DATA_DIR: root, LITTLEBOX_ACCOUNT_VERIFICATION: "required" };
}

function cli(...args: string[]) {
  const run = Bun.spawnSync([binary, ...args], { env: operatorEnv() });
  return { ok: run.exitCode === 0, out: run.stdout.toString(), err: run.stderr.toString() };
}

function issueCode(note = "integration") {
  const created = cli("creator-code-create", "--note", note);
  expect(created.ok).toBe(true);
  const code = created.out.match(/shown once: (\S+)/)![1];
  const id = created.out.match(/ID: (\S+)/)![1];
  return { code, id };
}

async function post(route: string, body: object, headers: Record<string, string> = {}) {
  return fetch(url + route, { method: "POST", redirect: "manual", headers: { "content-type": "application/json", "x-forwarded-for": freshClient(), ...headers }, body: JSON.stringify(body) });
}

async function mailbox(): Promise<{ to: string; purpose: string; link: string }[]> {
  const text = await readFile(path.join(root, "mailbox.jsonl"), "utf8").catch(() => "");
  return text.split("\n").filter(Boolean).map((line) => JSON.parse(line));
}

/** The queue's worker writes the development mailbox shortly after the request returns, so wait for it. */
async function lastLinkFor(email: string, seen = 0) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    const messages = (await mailbox()).filter((item) => item.to === email);
    if (messages.length > seen) return new URL(messages.at(-1)!.link);
    await Bun.sleep(100);
  }
  throw Error(`No link was sent to ${email}.`);
}

function tokenOf(link: URL) {
  const fragment = new URLSearchParams(link.hash.slice(1));
  return fragment.get("verification") ?? fragment.get("recovery")!;
}

async function verify(email: string) {
  const response = await post("/_littlebox/owner/verify", { token: tokenOf(await lastLinkFor(email)), password });
  expect(response.status).toBe(200);
  return response.headers.get("set-cookie")!.split(";")[0];
}

async function admission() {
  return (await (await fetch(`${url}/_littlebox/owner/config`)).json()).admission as string;
}

function withCookie(cookie: string) {
  return new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "littlebox-admission-"));
  const port = Number(process.env.LITTLEBOX_ADMISSION_TEST_PORT) || 19400 + Math.floor(Math.random() * 90);
  url = `http://127.0.0.1:${port}`;
  kernel = Bun.spawn([binary], {
    env: { ...operatorEnv(), LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_ADMIN_KEY: setupKey, LITTLEBOX_PUBLIC_ORIGIN: url, LITTLEBOX_HOST_SUFFIX: "localhost", LITTLEBOX_BUN_PATH: process.execPath, LITTLEBOX_TRUSTED_PROXIES: "127.0.0.1", LITTLEBOX_MAIL_TRANSPORT: "development", LITTLEBOX_MAIL_LIMIT_RECIPIENT_PER_MINUTE: "100", LITTLEBOX_MAIL_LIMIT_CLIENT_PER_HOUR: "1000" },
    stdout: "ignore", stderr: "inherit",
  });
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_littlebox/health`)).ok) return; } catch {}
    if (kernel.exitCode !== null) throw Error("Admission test Kernel stopped.");
    await Bun.sleep(100);
  }
  throw Error("Admission test Kernel did not start.");
}, 120_000);

afterAll(async () => { kernel?.kill(); if (kernel) await kernel.exited; if (root) await rm(root, { recursive: true, force: true }); });

describe("creator admission", () => {
  test("the installation key creates the first owner and no one after", async () => {
    expect(await admission()).toBe("setup-key");
    expect((await post("/_littlebox/owner/signup", { email: "operator@admission.test", setupKey })).status).toBe(202);
    await verify("operator@admission.test");
    expect(await admission()).toBe("code");
    const second = await post("/_littlebox/owner/signup", { email: "leaked@admission.test", setupKey });
    expect(second.status).toBe(403);
    const message = (await second.json()).error as string;
    expect(message).toContain("invitation code");
    expect(message.toLowerCase()).not.toContain("key");
    expect((await mailbox()).some((item) => item.to === "leaked@admission.test")).toBe(false);
  });

  let creatorCookie = "";
  let creatorToken = "";
  let codeId = "";
  const oauth = { args: {} as Record<string, string>, verifier: "v".repeat(64) };

  test("a creator admitted with a code verifies and lands back on the MCP client's consent", async () => {
    const issued = issueCode("For Ada");
    codeId = issued.id;
    expect(cli("creator-code-list").out).toContain(`${codeId}\topen\t`);

    const registration = await (await fetch(`${url}/oauth/register`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ client_name: "Admission agent", redirect_uris: ["http://127.0.0.1:9996/callback"] }) })).json();
    const challenge = Buffer.from(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(oauth.verifier))).toString("base64url");
    oauth.args = { client_id: registration.client_id, redirect_uri: registration.redirect_uris[0], response_type: "code", code_challenge_method: "S256", code_challenge: challenge, scope: "read", resource: `${url}/mcp`, state: "admission-state" };
    // A signed-out creator is sent to the owner page with the consent path.
    const start = await fetch(`${url}/oauth/authorize?${new URLSearchParams(oauth.args)}`, { redirect: "manual" });
    expect(start.status).toBe(303);
    const login = new URL(start.headers.get("location")!, url);
    expect(login.pathname).toBe("/_littlebox/owner/login");
    const returnTo = login.searchParams.get("returnTo")!;
    expect(returnTo.startsWith("/oauth/authorize?")).toBe(true);

    const signup = await post("/_littlebox/owner/signup", { email: "ada@admission.test", code: issued.code.toLowerCase(), returnTo });
    expect(signup.status).toBe(202);
    expect(signup.headers.get("set-cookie")).toBeNull();
    const link = await lastLinkFor("ada@admission.test");
    expect(link.pathname).toBe("/_littlebox/owner/login");
    expect(link.searchParams.get("returnTo")).toBe(returnTo);
    expect(cli("creator-code-list").out).toContain("ada@admission.test (pending)");

    creatorCookie = await verify("ada@admission.test");
    const consent = await fetch(url + returnTo, { headers: { cookie: creatorCookie } });
    expect(consent.status).toBe(200);
    const csrf = (await consent.text()).match(/name=csrf value="([^"]+)"/)![1];
    const approved = await fetch(`${url}/oauth/authorize`, { method: "POST", redirect: "manual", headers: { cookie: creatorCookie, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ ...oauth.args, csrf }) });
    expect(approved.status).toBe(303);
    const back = new URL(approved.headers.get("location")!);
    expect(back.origin + back.pathname).toBe("http://127.0.0.1:9996/callback");
    expect(back.searchParams.get("state")).toBe("admission-state");
    const exchanged = await fetch(`${url}/oauth/token`, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ grant_type: "authorization_code", client_id: oauth.args.client_id, redirect_uri: oauth.args.redirect_uri, code: back.searchParams.get("code")!, resource: oauth.args.resource, code_verifier: oauth.verifier }) });
    expect(exchanged.status).toBe(200);
    creatorToken = (await exchanged.json()).access_token;
    expect(await new McpClient(`${url}/mcp`, { token: creatorToken }).call("projects_list")).toMatchObject({ projects: [] });
    expect(cli("creator-code-list").out).toMatch(new RegExp(`${codeId}\\tused\\t.*ada@admission\\.test \\(active\\)`));
  });

  test("a used, revoked or unknown code and the key once codes exist are refused", async () => {
    const revoked = issueCode("Revoked before use");
    expect(cli("creator-code-revoke", revoked.id).out).toContain("Revoked code");
    expect((await post("/_littlebox/owner/signup", { email: "eve@admission.test", code: "0000000000000000000000000" })).status).toBe(403);
    expect((await post("/_littlebox/owner/signup", { email: "eve@admission.test", code: revoked.code })).status).toBe(403);
    expect((await post("/_littlebox/owner/signup", { email: "eve@admission.test", setupKey })).status).toBe(403);
    // A code is refused once used, for anyone else.
    const spent = issueCode("Spent twice");
    expect((await post("/_littlebox/owner/signup", { email: "bob@admission.test", code: spent.code })).status).toBe(202);
    expect((await post("/_littlebox/owner/signup", { email: "eve@admission.test", code: spent.code })).status).toBe(403);
    expect((await mailbox()).some((item) => item.to === "eve@admission.test")).toBe(false);
    expect(cli("creator-code-revoke", spent.id).ok).toBe(false);
  });

  test("suspending a creator ends their sessions, links, OAuth codes and MCP credentials", async () => {
    const owner = withCookie(creatorCookie);
    const delegated = (await owner.call<{ token: string }>("token_create", { scopes: ["read"], ttlSeconds: 3600 })).token;
    // An approved OAuth code the client has not exchanged yet.
    const consent = await fetch(`${url}/oauth/authorize?${new URLSearchParams(oauth.args)}`, { headers: { cookie: creatorCookie } });
    const csrf = (await consent.text()).match(/name=csrf value="([^"]+)"/)![1];
    const approved = await fetch(`${url}/oauth/authorize`, { method: "POST", redirect: "manual", headers: { cookie: creatorCookie, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ ...oauth.args, csrf }) });
    const pendingCode = new URL(approved.headers.get("location")!).searchParams.get("code")!;
    // An open recovery link.
    expect((await post("/_littlebox/owner/recover", { email: "ada@admission.test" })).status).toBe(202);
    const recovery = tokenOf(await lastLinkFor("ada@admission.test"));

    const suspended = cli("creator-account-suspend", "ada@admission.test");
    expect(suspended.ok).toBe(true);

    await expect(owner.call("projects_list")).rejects.toThrow();
    await expect(new McpClient(`${url}/mcp`, { token: creatorToken }).call("projects_list")).rejects.toThrow();
    await expect(new McpClient(`${url}/mcp`, { token: delegated }).call("projects_list")).rejects.toThrow();
    const exchange = await fetch(`${url}/oauth/token`, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ grant_type: "authorization_code", client_id: oauth.args.client_id, redirect_uri: oauth.args.redirect_uri, code: pendingCode, resource: oauth.args.resource, code_verifier: oauth.verifier }) });
    expect(exchange.status).toBe(400);
    expect((await post("/_littlebox/owner/reset", { token: recovery, password: "another password 2026" })).status).toBe(410);
    expect((await post("/_littlebox/owner/login", { email: "ada@admission.test", password })).status).toBe(403);
    expect(cli("creator-code-list").out).toContain("ada@admission.test (suspended)");

    // A creator admitted with a code but suspended before verifying loses the link.
    expect(cli("creator-account-suspend", "bob@admission.test").ok).toBe(true);
    expect((await post("/_littlebox/owner/verify", { token: tokenOf(await lastLinkFor("bob@admission.test")), password })).status).toBe(410);
  }, 30_000);
});
