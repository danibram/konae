import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { Database } from "bun:sqlite";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient, checksum, encodeBase64 } from "../src/mcp/client";
import { signupModes } from "../src/deployments/client";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import type { OAuthClientProvider } from "@modelcontextprotocol/sdk/client/auth.js";

let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let url: string;
let port: number;
let alice: McpClient;
let bob: McpClient;
let aliceCookie: string;
const setupKey = "mcp-integration-setup-secret";
const password = "integration-password-2026";
// Sign-in is rate limited per client address. The Kernel trusts X-Forwarded-For
// from 127.0.0.1 here, so each helper call can present its own address.
let nextClient = 0;
const freshClient = () => { nextClient++; return `198.18.${Math.floor(nextClient / 250)}.${nextClient % 250 + 1}`; };

async function owner(email: string) {
  const response = await fetch(`${url}/_littlebox/owner/signup`, { method: "POST", headers: { "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email, password, setupKey }) });
  expect(response.status).toBe(200);
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  const client = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  return { client, cookie };
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "littlebox-mcp-"));
  port = Number(process.env.LITTLEBOX_MCP_TEST_PORT) || 19300 + Math.floor(Math.random() * 90);
  url = `http://127.0.0.1:${port}`;
  await startKernel();
  const first = await owner("alice@owner.test"); alice = first.client; aliceCookie = first.cookie;
  bob = (await owner("bob@owner.test")).client;
}, 120_000);

async function startKernel() {
  kernel = Bun.spawn(["kernel-rs/target/debug/littlebox-kernel"], {
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_S3_"))), LITTLEBOX_DATA_DIR: root, LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_ADMIN_KEY: setupKey, LITTLEBOX_PUBLIC_ORIGIN: url, LITTLEBOX_HOST_SUFFIX: "localhost", LITTLEBOX_BUN_PATH: process.execPath, LITTLEBOX_TRUSTED_PROXIES: "127.0.0.1", LITTLEBOX_MAIL_TRANSPORT: "development" },
    stdout: "ignore", stderr: "inherit",
  });
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_littlebox/health`)).ok) return; } catch {}
    if (kernel.exitCode !== null) throw Error("MCP test Kernel stopped.");
    await Bun.sleep(100);
  }
  throw Error("MCP test Kernel did not start.");
}

afterAll(async () => { kernel?.kill(); if (kernel) await kernel.exited; if (root) await rm(root, { recursive: true, force: true }); });

function payload(id: string, visibility: "public" | "private" = "public") {
  return {
    project: { id, name: "MCP proof", slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility }, files: {
      "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, title TEXT NOT NULL, user_id TEXT NOT NULL);",
      "actions.json": JSON.stringify({ version: 1, actions: {
        "notes.list": { kind: "query", access: "authenticated", statement: "SELECT id,title FROM notes WHERE user_id=?1 LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
        "notes.create": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(title,user_id) VALUES(?1,?2)", bindings: [{ source: "input", key: "title", type: "string", maxLength: 100 }, { source: "actor", field: "id" }] },
        "notes.count": { kind: "query", access: "anonymous", statement: "SELECT COUNT(*) AS total FROM notes", bindings: [] },
      } }),
    } }, visibility, artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>MCP proof</main>" } },
  };
}

async function release(id: string, visibility: "public" | "private" = "public") {
  await alice.call("project_create", { projectId: id, name: "MCP proof" });
  const transfer = await alice.upload(new TextEncoder().encode(JSON.stringify(payload(id, visibility))), id);
  const plan = await alice.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
  const receipt = await alice.call<{ id: string; hostname: string; url: string }>("release_apply", { planId: plan.planId });
  await alice.cancel(transfer.transferId);
  return { receipt, plan };
}

function signup(hostname: string, email: string) {
  return fetch(`${url}/_littlebox/auth/signup`, { method: "POST", headers: { host: hostname, "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email, password }) });
}

async function appUser(hostname: string, email: string) {
  const response = await signup(hostname, email);
  expect(response.status).toBe(200);
  return runtimeClient(hostname, response.headers.get("set-cookie")!.split(";")[0]);
}

function runtimeClient(hostname: string, cookie?: string) {
  return new McpClient(`${url}/_littlebox/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", hostname); if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

async function loginPage(hostname: string) {
  return (await fetch(`${url}/_littlebox/login`, { headers: { host: hostname } })).text();
}

describe("authenticated MCP", () => {
  test("Managed sources persist per owner and reject stale writes", async () => {
    const id = "managed_sources_proof";
    await alice.call("project_create", { projectId: id, name: "Managed", sourceAuthority: "managed" });
    const project = { id, name: "Managed", slug: "managed", files: { "index.html": "first" }, directories: [], databaseModel: { source: "", acceptedSource: "", revision: "" }, manifest: { version: 1, visibility: "private", runtime: { database: { mode: "declared-actions", actions: [] }, auth: { mode: "required" }, files: { access: "deployment" } } }, updatedAt: 1 };
    const upload = await alice.upload(new TextEncoder().encode(JSON.stringify(project)), id);
    expect(await alice.call("project_sources_save", { projectId: id, transferId: upload.transferId, expectedRevision: 0 })).toEqual({ revision: 1 });
    await expect(alice.call("project_sources_save", { projectId: id, transferId: upload.transferId, expectedRevision: 0 })).rejects.toThrow("revision conflict");
    await expect(bob.call("project_sources_read", { projectId: id })).rejects.toThrow("access denied");
    await expect(bob.call("project_sources_save", { projectId: id, transferId: upload.transferId, expectedRevision: 1 })).rejects.toThrow();
    const saved = await alice.call<{ revision: number; transfer: Parameters<McpClient["download"]>[0] }>("project_sources_read", { projectId: id });
    expect(saved.revision).toBe(1);
    expect(JSON.parse(new TextDecoder().decode(await alice.download(saved.transfer)))).toEqual(project);
    const invalidUpload = await alice.upload(new TextEncoder().encode(JSON.stringify({ ...project, files: { "index.html": 123 } })), id);
    await expect(alice.call("project_sources_save", { projectId: id, transferId: invalidUpload.transferId, expectedRevision: 1 })).rejects.toThrow("Invalid project files");
    await alice.cancel(invalidUpload.transferId);
    const unchanged = await alice.call<{ revision: number; transfer: Parameters<McpClient["download"]>[0] }>("project_sources_read", { projectId: id });
    expect(unchanged.revision).toBe(1);
    expect(JSON.parse(new TextDecoder().decode(await alice.download(unchanged.transfer)))).toEqual(project);
    const changed = { ...project, files: { "index.html": "changed" } };
    const secondUpload = await alice.upload(new TextEncoder().encode(JSON.stringify(changed)), id);
    const writes = await Promise.allSettled([
      alice.call("project_sources_save", { projectId: id, transferId: upload.transferId, expectedRevision: 1 }),
      alice.call("project_sources_save", { projectId: id, transferId: secondUpload.transferId, expectedRevision: 1 }),
    ]);
    expect(writes.filter((result) => result.status === "fulfilled")).toHaveLength(1);
    expect(writes.filter((result) => result.status === "rejected")).toHaveLength(1);
    await alice.cancel(secondUpload.transferId);
    await alice.cancel(upload.transferId);
    const writeGrant = await alice.call<{ token: string }>("token_create", { scopes: ["write"], projectIds: [id], ttlSeconds: 60 });
    const writer = new McpClient(`${url}/mcp`, { token: writeGrant.token });
    const writerUpload = await writer.upload(new TextEncoder().encode(JSON.stringify(project)), id);
    expect(await writer.call("project_sources_save", { projectId: id, transferId: writerUpload.transferId, expectedRevision: 2 })).toEqual({ revision: 3 });
    await writer.cancel(writerUpload.transferId);
    const readGrant = await alice.call<{ token: string }>("token_create", { scopes: ["read"], projectIds: [id], ttlSeconds: 60 });
    const reader = new McpClient(`${url}/mcp`, { token: readGrant.token });
    await expect(reader.call("project_sources_save", { projectId: id, transferId: upload.transferId, expectedRevision: 3 })).rejects.toThrow("Unknown tool");

    const external = "external_sources_proof";
    await alice.call("project_create", { projectId: external, name: "External" });
    await expect(alice.call("project_sources_read", { projectId: external })).rejects.toThrow("externally");
    const externalUpload = await alice.upload(new TextEncoder().encode(JSON.stringify({ ...project, id: external })), external);
    await expect(alice.call("project_sources_save", { projectId: external, transferId: externalUpload.transferId, expectedRevision: 0 })).rejects.toThrow("externally");
    await alice.cancel(externalUpload.transferId);
  });

  test("app sessions and data do not cross deployment boundaries", async () => {
    const alpha = (await release("isolation_alpha")).receipt;
    const beta = (await release("isolation_beta")).receipt;
    const alphaSignup = await signup(alpha.hostname, "same-user@app.test");
    expect(alphaSignup.status).toBe(200);
    const alphaCookie = alphaSignup.headers.get("set-cookie")!.split(";")[0];
    const alphaUser = runtimeClient(alpha.hostname, alphaCookie);
    const betaWithAlphaCookie = runtimeClient(beta.hostname, alphaCookie);
    const betaUser = await appUser(beta.hostname, "same-user@app.test");

    await alphaUser.call("actions_invoke", { name: "notes.create", input: { title: "Alpha private row" } });
    await betaUser.call("actions_invoke", { name: "notes.create", input: { title: "Beta private row" } });
    expect(await alphaUser.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Alpha private row" }]);
    expect(await betaUser.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Beta private row" }]);
    await expect(betaWithAlphaCookie.call("actions_invoke", { name: "notes.list" })).rejects.toThrow("authentication-required");
    await expect(alphaUser.call("actions_invoke", { name: "notes.list", deploymentId: beta.id })).rejects.toThrow();
    await expect(bob.call("database_execute", { deploymentId: alpha.id, operation: "inspect", input: {} })).rejects.toThrow("access denied");
  });

  test("nullable action inputs reach SQL as null without relaxing required inputs", async () => {
    const id = "nullable_inputs_proof";
    await alice.call("project_create", { projectId: id, name: "Nullable input proof" });
    const request = payload(id);
    const program = JSON.parse(request.project.files["actions.json"]);
    program.actions["inputs.echo"] = {
      kind: "query", access: "anonymous", statement: "SELECT ?1 AS text_value, ?2 AS number_value, ?3 AS boolean_value",
      bindings: [
        { source: "input", key: "q", type: "string", maxLength: 4, nullable: true },
        { source: "input", key: "limit", type: "number", nullable: true },
        { source: "input", key: "done", type: "boolean", nullable: true },
      ],
    };
    request.project.files["actions.json"] = JSON.stringify(program);
    const transfer = await alice.upload(new TextEncoder().encode(JSON.stringify(request)), id);
    let receipt: { hostname: string };
    try {
      const plan = await alice.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
      receipt = await alice.call("release_apply", { planId: plan.planId });
    } finally { await alice.cancel(transfer.transferId); }
    const visitor = runtimeClient(receipt.hostname);
    const echo = (input?: unknown) => visitor.call("actions_invoke", { name: "inputs.echo", ...(input === undefined ? {} : { input }) });
    expect(await echo()).toEqual([{ text_value: null, number_value: null, boolean_value: null }]);
    expect(await echo({ q: null, limit: null, done: null })).toEqual([{ text_value: null, number_value: null, boolean_value: null }]);
    expect(await echo({ q: "ship", limit: 0, done: false })).toEqual([{ text_value: "ship", number_value: 0, boolean_value: 0 }]);
    expect(await echo({ q: "ship", limit: 2.5, done: true })).toEqual([{ text_value: "ship", number_value: 2.5, boolean_value: 1 }]);
    for (const input of [{ q: "" }, { q: "longer" }, { q: 5 }, { limit: "5" }, { done: 0 }, { other: null }]) {
      await expect(echo(input)).rejects.toThrow();
    }
    const member = await appUser(receipt.hostname, "nullable@app.test");
    for (const input of [{}, { title: null }, { title: "" }]) {
      await expect(member.call("actions_invoke", { name: "notes.create", input })).rejects.toThrow();
    }
    await member.call("actions_invoke", { name: "notes.create", input: { title: "Required title" } });
    expect(await member.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Required title" }]);
  });

  test("an official MCP client discovers and executes tools over Streamable HTTP", async () => {
    const token = await alice.call<{ token: string }>("token_create", { scopes: ["read"], ttlSeconds: 60 });
    const client = new Client({ name: "independent-agent", version: "1.0.0" });
    const transport = new StreamableHTTPClientTransport(new URL(`${url}/mcp`), { requestInit: { headers: { authorization: `Bearer ${token.token}` } } });
    try {
      await client.connect(transport);
      expect((await client.listTools()).tools.some(tool => tool.name === "projects_list")).toBe(true);
      const result = await client.callTool({ name: "platform_describe", arguments: {} });
      expect(result.isError).toBe(false);
      expect(result.structuredContent).toMatchObject({ result: { name: "Littlebox" } });
    } finally { await client.close(); }
  });

  test("portable imports keep closed and allowlist signup policies from the first request", async () => {
    for (const mode of ["closed", "allowlist"]) {
      const sourceId = `policy_source_${mode}`;
      const { receipt: source } = await release(sourceId);
      const member = await appUser(source.hostname, "existing@app.test");
      await member.call("actions_invoke", { name: "notes.create", input: { title: `Kept ${mode}` } });
      const policy = { mode, allowlist: mode === "allowlist" ? ["allowed@app.test"] : [] };
      await alice.call("signup_policy_set", { deploymentId: source.id, ...policy });
      const exported = await alice.call<any>("project_export", { deploymentId: source.id });
      const portable = JSON.parse(new TextDecoder().decode(await alice.download(exported))).result;
      expect(portable.initialSignupPolicy).toEqual(policy);
      expect(portable.visibility).toBe("public");
      const targetId = `policy_target_${mode}`;
      await alice.call("project_create", { projectId: targetId, name: "Imported signup policy" });
      const targetRequest = { ...payload(targetId), ...portable };
      const upload = await alice.upload(new TextEncoder().encode(JSON.stringify(targetRequest)), targetId);
      let target: { id: string; hostname: string };
      try {
        const plan = await alice.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
        target = await alice.call("release_apply", { planId: plan.planId });
      } finally { await alice.cancel(upload.transferId); }
      expect(await alice.call("signup_policy_read", { deploymentId: target.id })).toMatchObject(policy);
      expect((await signup(target.hostname, "stranger@app.test")).status).toBe(403);
      expect((await signup(target.hostname, "allowed@app.test")).status).toBe(mode === "allowlist" ? 200 : 403);
      const login = await fetch(`${url}/_littlebox/auth/login`, { method: "POST", headers: { host: target.hostname, "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email: "existing@app.test", password }) });
      expect(login.status).toBe(200);
      const importedMember = runtimeClient(target.hostname, login.headers.get("set-cookie")!.split(";")[0]);
      expect(await importedMember.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: `Kept ${mode}` }]);
      const overwrite = { ...payload(targetId), initialSignupPolicy: { mode: "open", allowlist: [] } };
      const rejected = await alice.upload(new TextEncoder().encode(JSON.stringify(overwrite)), targetId);
      try {
        await expect(alice.call("release_plan", { transferId: rejected.transferId })).rejects.toThrow("first deployment");
      } finally { await alice.cancel(rejected.transferId); }
      await release(targetId);
      expect(await alice.call("signup_policy_read", { deploymentId: target.id })).toMatchObject(policy);
    }
  }, 30_000);

  test("portable signup policies validate and normalize their entries before publishing", async () => {
    const id = "invalid_initial_policy";
    await alice.call("project_create", { projectId: id, name: "Initial policy validation" });
    for (const initialSignupPolicy of [
      { mode: "invalid", allowlist: [] },
      { mode: "allowlist", allowlist: ["not an email"] },
      { mode: "allowlist", allowlist: Array.from({ length: 101 }, (_, i) => `user${i}@app.test`) },
    ]) {
      const transfer = await alice.upload(new TextEncoder().encode(JSON.stringify({ ...payload(id), initialSignupPolicy })), id);
      try { await expect(alice.call("release_plan", { transferId: transfer.transferId })).rejects.toThrow(); }
      finally { await alice.cancel(transfer.transferId); }
    }
    const initialSignupPolicy = { mode: "allowlist", allowlist: [" ALLOWED@APP.TEST ", "allowed@app.test", "@TEAM.TEST"] };
    const transfer = await alice.upload(new TextEncoder().encode(JSON.stringify({ ...payload(id), initialSignupPolicy })), id);
    try {
      const plan = await alice.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
      const receipt = await alice.call<{ id: string }>("release_apply", { planId: plan.planId });
      expect(await alice.call("signup_policy_read", { deploymentId: receipt.id })).toMatchObject({ mode: "allowlist", allowlist: ["allowed@app.test", "@team.test"] });
    } finally { await alice.cancel(transfer.transferId); }
  });

  test("portable exports capture visibility after an earlier project inspection becomes stale", async () => {
    const id = "portable_visibility_snapshot";
    const { receipt } = await release(id);
    const inspected = await alice.call<any>("project_inspect", { projectId: id });
    expect(inspected.deployment.manifest.visibility).toBe("public");
    await release(id, "private");
    const exported = await alice.call<any>("project_export", { deploymentId: receipt.id });
    const portable = JSON.parse(new TextDecoder().decode(await alice.download(exported))).result;
    expect(portable.visibility).toBe("private");
    expect(portable.initialSignupPolicy.mode).toBe("closed");
    expect(inspected.deployment.manifest.visibility).toBe("public");
  });

  test("publication rejects mutation RETURNING and accepts it inside a string", async () => {
    const id = "returning_validation_proof";
    await alice.call("project_create", { projectId: id, name: "Mutation validation proof" });
    const request = payload(id);
    const program = JSON.parse(request.project.files["actions.json"]);
    program.actions["notes.literal"] = { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(title,user_id) VALUES('RETURNING',?1) RETURNING id", bindings: [{ source: "actor", field: "id" }] };
    request.project.files["actions.json"] = JSON.stringify(program);
    const rejected = await alice.upload(new TextEncoder().encode(JSON.stringify(request)), id);
    try {
      await expect(alice.call("release_plan", { transferId: rejected.transferId })).rejects.toThrow("cannot use RETURNING");
    } finally { await alice.cancel(rejected.transferId); }
    program.actions["notes.literal"].statement = "INSERT INTO notes(title,user_id) VALUES('RETURNING',?1) /* RETURNING id */";
    request.project.files["actions.json"] = JSON.stringify(program);
    const accepted = await alice.upload(new TextEncoder().encode(JSON.stringify(request)), id);
    let receipt: { hostname: string };
    try {
      const plan = await alice.call<{ planId: string }>("release_plan", { transferId: accepted.transferId });
      receipt = await alice.call("release_apply", { planId: plan.planId });
    } finally { await alice.cancel(accepted.transferId); }
    const member = await appUser(receipt.hostname, "returning@app.test");
    expect(await member.call("actions_invoke", { name: "notes.list" })).toEqual([]);
    expect(await member.call("actions_invoke", { name: "notes.literal" })).toMatchObject({ changes: 1 });
    expect(await member.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "RETURNING" }]);
  });

  test("the local CLI creates, builds, publishes and develops without mutating hosted data", async () => {
    const directory = path.join(root, "external-source");
    const token = await alice.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish"], ttlSeconds: 60 });
    const env = { ...process.env, LITTLEBOX_MCP_TOKEN: token.token };
    const run = async (command: string, args: string[] = []) => {
      const process = Bun.spawn([Bun.which("bun")!, "run", "scripts/littlebox.ts", command, "--dir", directory, ...args], { env, stdout: "pipe", stderr: "pipe" });
      const [code, output, error] = await Promise.all([process.exited, new Response(process.stdout).text(), new Response(process.stderr).text()]);
      if (code !== 0) throw Error(error);
      return output;
    };
    expect(await run("init", ["--url", url, "--name", "External proof"])).toContain("Created External proof");
    expect(await run("build")).toContain("artifact.json");
    expect(await run("publish")).toContain("http://external-proof-");
    const config = await Bun.file(path.join(directory, "littlebox.json")).json();
    expect(config.project.sourceAuthority).toBe("external");
    const state = await alice.call<{ revision: number }>("project_inspect", { projectId: config.project.id });
    expect(state.revision).toBe(1);
    const port = String(18950 + Math.floor(Math.random() * 50));
    const dev = Bun.spawn([process.execPath, "run", "scripts/littlebox.ts", "dev", "--dir", directory, "--port", port], {
      env: { ...env, LITTLEBOX_MCP_TOKEN: "must-not-be-used-in-development", LITTLEBOX_S3_ENDPOINT: "http://127.0.0.1:1", LITTLEBOX_MCP_ORIGIN: "http://unused-origin.test" },
      stdout: "pipe", stderr: "inherit",
    });
    let timer: ReturnType<typeof setTimeout>;
    try {
      const output = (async () => {
        let text = "";
        for await (const block of dev.stdout) { text += new TextDecoder().decode(block); if (/http:\/\/external-proof-[a-f0-9]+\.localhost:\d+/.test(text)) return text; }
        throw Error("Development process returned no URL.");
      })();
      const result = await Promise.race([output, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(Error("Development CLI timed out.")), 20_000); })]);
      expect(result).toContain(`.localhost:${port}`);
      expect(await Bun.file(path.join(directory, ".littlebox/dev-data/owners.sqlite")).exists()).toBe(true);
      expect((await alice.call<{ revision: number }>("project_inspect", { projectId: config.project.id })).revision).toBe(1);
    } finally { clearTimeout(timer!); dev.kill(); await dev.exited; }
  }, 30_000);
  test("requires credentials before discovery and closes the old operation routes", async () => {
    expect((await fetch(`${url}/mcp`, { method: "POST", body: "{}" })).status).toBe(401);
    const stale = await fetch(`${url}/_littlebox/admin/deploy`, { method: "POST", headers: { "x-littlebox-admin-key": setupKey } });
    expect(stale.status).toBe(410);
    for (const route of ["/_littlebox/session", "/_littlebox/files", "/_littlebox/actions/notes.list"]) expect((await fetch(url + route)).status).toBe(410);
    expect((await alice.call<{ protocolVersion: string }>("platform_describe")).protocolVersion).toBe("2025-11-25");
  });

  test("rejects cross-origin requests and unknown DNS hosts", async () => {
    for (const headers of [{ cookie: aliceCookie, origin: "https://attacker.test" }, { cookie: aliceCookie, origin: "http://127.0.0.1:1" }, { cookie: aliceCookie, host: "attacker.test" }]) {
      expect((await fetch(`${url}/mcp`, { method: "POST", headers, body: "{}" })).status).toBe(403);
    }
  });

  test("isolates projects, enforces scoped credentials and revokes tokens", async () => {
    await alice.call("project_create", { projectId: "owned_alpha", name: "Alpha" });
    await alice.call("project_create", { projectId: "owned_beta", name: "Beta" });
    await expect(bob.call("project_inspect", { projectId: "owned_alpha" })).rejects.toThrow("access denied");
    await expect(bob.call("project_create", { projectId: "owned_alpha", name: "Steal" })).rejects.toThrow("access denied");
    const token = await alice.call<{ token: string; tokenId: string }>("token_create", { scopes: ["read"], projectIds: ["owned_alpha"], ttlSeconds: 60 });
    const scoped = new McpClient(`${url}/mcp`, { token: token.token });
    expect((await scoped.call<{ projects: unknown[] }>("projects_list")).projects).toHaveLength(1);
    await expect(scoped.call("project_inspect", { projectId: "owned_beta" })).rejects.toThrow("outside");
    await expect(scoped.call("project_create", { name: "Forbidden" })).rejects.toThrow("Unknown tool");
    await expect(scoped.call("token_create", { scopes: ["read"] })).rejects.toThrow("Unknown tool");
    await alice.call("token_revoke", { tokenId: token.tokenId });
    await expect(scoped.call("projects_list")).rejects.toThrow("Sign in");
  });

  test("checks transfer ownership, offsets, retries, checksums and immutable completion", async () => {
    const bytes = new TextEncoder().encode("An upload that survives a retry");
    const transfer = await alice.call<{ transferId: string }>("transfer_begin", { size: bytes.length, sha256: await checksum(bytes) });
    const args = { transferId: transfer.transferId, offset: 0, data: encodeBase64(bytes) };
    await expect(bob.call("transfer_status", { transferId: transfer.transferId })).rejects.toThrow("not found");
    await alice.call("transfer_write", args); await alice.call("transfer_write", args);
    await expect(alice.call("transfer_write", { ...args, data: encodeBase64(new TextEncoder().encode("x")) })).rejects.toThrow("does not match");
    await alice.call("transfer_complete", { transferId: transfer.transferId });
    await expect(alice.call("transfer_write", args)).rejects.toThrow("immutable");
    await alice.cancel(transfer.transferId);
    const invalid = await alice.call<{ transferId: string }>("transfer_begin", { size: bytes.length, sha256: "0".repeat(64) });
    await alice.call("transfer_write", { ...args, transferId: invalid.transferId });
    await expect(alice.call("transfer_complete", { transferId: invalid.transferId })).rejects.toThrow("mismatch");
    await alice.cancel(invalid.transferId);
  });

  test("publishes, rejects stale plans, and returns the same receipt on retry", async () => {
    const id = "release_proof";
    await alice.call("project_create", { projectId: id, name: "Release" });
    const upload = await alice.upload(new TextEncoder().encode(JSON.stringify(payload(id))), id);
    const one = await alice.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
    const two = await alice.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
    const first = await alice.call("release_apply", { planId: one.planId });
    expect(await alice.call("release_apply", { planId: one.planId })).toEqual(first);
    await expect(alice.call("release_apply", { planId: two.planId })).rejects.toThrow("revision changed");
    await expect(bob.call("release_apply", { planId: one.planId })).rejects.toThrow();
    await alice.cancel(upload.transferId);
  });

  test("separates app users, deployments and administrative capabilities", async () => {
    const { receipt } = await release("runtime_proof");
    const user = await appUser(receipt.hostname, "alice@app.test");
    const other = await appUser(receipt.hostname, "bob@app.test");
    await user.call("actions_invoke", { name: "notes.create", input: { title: "Only Alice" } });
    expect(await user.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Only Alice" }]);
    expect(await other.call("actions_invoke", { name: "notes.list" })).toEqual([]);
    await expect(user.call("database_execute", { deploymentId: receipt.id, operation: "inspect" })).rejects.toThrow("Unknown tool");
    await expect(bob.call("database_execute", { deploymentId: receipt.id, operation: "inspect" })).rejects.toThrow("access denied");
    const file = new TextEncoder().encode("Private attachment");
    const upload = await user.upload(file);
    const saved = await user.call<{ id: string; visibility: string; ownerId: string }>("files_upload", { transferId: upload.transferId, name: "note.txt", type: "text/plain" });
    expect(saved.visibility).toBe("private");
    expect(await user.call("files_upload", { transferId: upload.transferId, name: "note.txt", type: "text/plain" })).toEqual(saved);
    await user.cancel(upload.transferId);
    expect(await other.call("files_list")).toEqual([]);
    await expect(other.call("files_read", { fileId: saved.id })).rejects.toThrow("File not found");
    const sharedBytes = new TextEncoder().encode("shared");
    const sharedUpload = await user.upload(sharedBytes);
    const shared = await user.call<{ id: string }>("files_upload", { transferId: sharedUpload.transferId, name: "shared.txt", type: "text/plain", visibility: "public" });
    await user.cancel(sharedUpload.transferId);
    const sharedList = await other.call<Array<{ id: string; visibility: string; ownerId?: string }>>("files_list");
    expect(sharedList).toEqual([expect.objectContaining({ id: shared.id, visibility: "public" })]);
    expect(sharedList[0].ownerId).toBeUndefined();
    expect(await other.download(await other.call<any>("files_read", { fileId: shared.id }))).toEqual(sharedBytes);
    const download = await user.call<any>("files_read", { fileId: saved.id });
    expect(await user.download(download)).toEqual(file);
    const exported = await alice.call<any>("database_export", { deploymentId: receipt.id });
    expect(new TextDecoder().decode((await alice.download(exported)).slice(0, 15))).toBe("SQLite format 3");
    await expect(alice.call("database_execute", { deploymentId: receipt.id, operation: "sql", input: { sql: "ATTACH DATABASE '/tmp/other.sqlite' AS other;" } })).rejects.toThrow();
    await user.call("auth_logout");
    // The revoked cookie now reads as a signed-out visitor of this public app.
    expect(await user.call("auth_session")).toMatchObject({ role: "anonymous" });
    await expect(user.call("actions_invoke", { name: "notes.list" })).rejects.toThrow("authentication-required");
  });

  test("the owner chooses who may create an account on a public app", async () => {
    const { receipt } = await release("signup_policy_proof");
    expect(await alice.call("signup_policy_read", { deploymentId: receipt.id })).toMatchObject({ mode: "open", allowlist: [], visibility: "public" });
    const member = await appUser(receipt.hostname, "first@app.test");
    expect(await loginPage(receipt.hostname)).toContain("/_littlebox/auth/signup");

    await alice.call("signup_policy_set", { deploymentId: receipt.id, mode: "closed" });
    expect((await signup(receipt.hostname, "late@app.test")).status).toBe(403);
    const closedPage = await loginPage(receipt.hostname);
    expect(closedPage).not.toContain("/_littlebox/auth/signup");
    expect(closedPage).not.toContain("Account creation is open");

    const policy = await alice.call("signup_policy_set", { deploymentId: receipt.id, mode: "allowlist", allowlist: ["Carol@Example.test", "@Team.test"] });
    expect(policy).toMatchObject({ mode: "allowlist", allowlist: ["carol@example.test", "@team.test"] });
    expect((await signup(receipt.hostname, "carol@EXAMPLE.TEST")).status).toBe(200);
    expect((await signup(receipt.hostname, "anyone@team.test")).status).toBe(200);
    expect((await signup(receipt.hostname, "dave@example.test")).status).toBe(403);
    expect((await signup(receipt.hostname, "anyone@sub.team.test")).status).toBe(403);
    await expect(alice.call("signup_policy_set", { deploymentId: receipt.id, mode: "allowlist", allowlist: ["not an email"] })).rejects.toThrow("not an email address");

    await expect(member.call("signup_policy_set", { deploymentId: receipt.id, mode: "open" })).rejects.toThrow("Unknown tool");
    await expect(member.call("signup_policy_read", { deploymentId: receipt.id })).rejects.toThrow("Unknown tool");
    await expect(runtimeClient(receipt.hostname).call("signup_policy_set", { deploymentId: receipt.id, mode: "open" })).rejects.toThrow("Unknown tool");
    await expect(bob.call("signup_policy_read", { deploymentId: receipt.id })).rejects.toThrow("access denied");
    const reader = await alice.call<{ token: string }>("token_create", { scopes: ["read"], ttlSeconds: 60 });
    const scoped = new McpClient(`${url}/mcp`, { token: reader.token });
    expect(await scoped.call("signup_policy_read", { deploymentId: receipt.id })).toMatchObject({ mode: "allowlist" });
    await expect(scoped.call("signup_policy_set", { deploymentId: receipt.id, mode: "open" })).rejects.toThrow("Unknown tool");
    // An app host never serves the owner MCP endpoint, so app cookies cannot reach owner tools.
    expect((await fetch(`${url}/mcp`, { method: "POST", headers: { host: receipt.hostname }, body: "{}" })).status).toBe(403);
  });

  test("an invited person enters an Invitation only app, and only manage_users decides who may join", async () => {
    const tools = await (await fetch(`${url}/mcp`, { method: "POST", headers: { cookie: aliceCookie, "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }) })).json();
    const policyTool = tools.result.tools.find((tool: { name: string }) => tool.name === "signup_policy_set");
    expect(policyTool.inputSchema.properties.mode.enum).toEqual([...signupModes]);

    const id = "invitation_proof";
    await alice.call("project_create", { projectId: id, name: "Invites" });
    const grant = async (scopes: string[], projectIds = [id]) => new McpClient(`${url}/mcp`, { token: (await alice.call<{ token: string }>("token_create", { scopes, projectIds, ttlSeconds: 300 })).token });
    const publisher = await grant(["read", "publish"]);
    const manager = await grant(["read", "manage_users"]);
    const publishWith = async (client: McpClient, body: object) => {
      const transfer = await client.upload(new TextEncoder().encode(JSON.stringify(body)), id);
      try {
        const plan = await client.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
        return await client.call<{ id: string; hostname: string }>("release_apply", { planId: plan.planId });
      } finally { await client.cancel(transfer.transferId).catch(() => {}); }
    };

    // publish may choose the policy of a first release, and the first request finds it.
    const receipt = await publishWith(publisher, { ...payload(id, "private"), initialSignupPolicy: { mode: "invitation", allowlist: [] } });
    expect(await publisher.call("signup_policy_read", { deploymentId: receipt.id })).toMatchObject({ mode: "invitation", visibility: "private" });
    expect((await signup(receipt.hostname, "stranger@app.test")).status).toBe(403);
    const page = await (await fetch(`${url}/_littlebox/login`, { headers: { host: receipt.hostname } })).text();
    expect(page).toContain("Only invited people can create an account");
    expect(page).not.toContain("/_littlebox/auth/signup");

    // Without manage_users, publish changes who may join neither directly nor through a republish.
    await expect(publisher.call("signup_policy_set", { deploymentId: receipt.id, mode: "open" })).rejects.toThrow("Unknown tool");
    await expect(publisher.call("app_invitations_create", { deploymentId: receipt.id, emails: ["x@app.test"], idempotencyKey: "publisher-key" })).rejects.toThrow("Unknown tool");
    await expect(publishWith(publisher, { ...payload(id, "public"), initialSignupPolicy: { mode: "open", allowlist: [] } })).rejects.toThrow("first deployment");
    await publishWith(publisher, payload(id, "private"));
    expect(await alice.call("signup_policy_read", { deploymentId: receipt.id })).toMatchObject({ mode: "invitation" });
    // A token from before manage_users existed does not gain it.
    const legacy = await grant(["read", "write", "publish"]);
    await expect(legacy.call("app_users_list", { deploymentId: receipt.id })).rejects.toThrow("Unknown tool");

    // A project-limited token cannot touch another project's users.
    const other = (await release("invitation_other")).receipt;
    await expect(manager.call("app_users_list", { deploymentId: other.id })).rejects.toThrow("outside this token's grants");
    await expect(manager.call("app_invitations_create", { deploymentId: other.id, emails: ["x@app.test"], idempotencyKey: "other-project" })).rejects.toThrow("outside this token's grants");
    await expect(bob.call("app_users_list", { deploymentId: receipt.id })).rejects.toThrow("access denied");

    const request = { deploymentId: receipt.id, emails: ["Ana@App.test"], idempotencyKey: "invite-ana-0001" };
    const created = await manager.call<{ results: Array<{ status: string; delivery: string; invitationId: string }> }>("app_invitations_create", request);
    expect(created.results[0]).toMatchObject({ status: "invited", delivery: "queued" });
    expect(await manager.call("app_invitations_create", request)).toEqual(created);
    const mailbox = (await Bun.file(path.join(root, "mailbox.jsonl")).text()).trim().split("\n").map((line) => JSON.parse(line)).filter((message) => message.purpose === "invitation");
    expect(mailbox).toHaveLength(1);
    expect(mailbox[0]).toMatchObject({ to: "ana@app.test", scope: `deployment:${receipt.id}` });
    const token = new URL(mailbox[0].link).hash.slice("#invitation=".length);
    expect(JSON.stringify(created)).not.toContain(token);

    const opened = await fetch(`${url}/_littlebox/invitation`, { headers: { host: receipt.hostname } });
    expect(opened.status).toBe(200);
    const post = (route: string, body: object) => fetch(`${url}${route}`, { method: "POST", headers: { host: receipt.hostname, "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify(body) });
    expect((await (await post("/_littlebox/auth/invitation", { token })).json()).result).toEqual({ email: "ana@app.test", app: "MCP proof", status: "new-account" });
    expect((await post("/_littlebox/auth/accept", { token: "0".repeat(64), password })).status).toBe(410);
    const accepted = await post("/_littlebox/auth/accept", { token, password });
    expect(accepted.status).toBe(200);
    const member = runtimeClient(receipt.hostname, accepted.headers.get("set-cookie")!.split(";")[0]);
    expect(await member.call("auth_session")).toMatchObject({ name: "ana@app.test" });
    await member.call("actions_invoke", { name: "notes.create", input: { title: "Invited" } });
    expect((await post("/_littlebox/auth/accept", { token, password })).status).toBe(410);

    const users = await manager.call<{ users: Array<{ userId: string; state: string; verified: boolean }> }>("app_users_list", { deploymentId: receipt.id });
    expect(users.users).toEqual([expect.objectContaining({ email: "ana@app.test", state: "active", verified: true })]);
    expect((await manager.call<{ invitations: Array<{ status: string }> }>("app_invitations_list", { deploymentId: receipt.id })).invitations[0].status).toBe("accepted");
    await manager.call("app_user_suspend", { deploymentId: receipt.id, userId: users.users[0].userId });
    await expect(member.call("auth_session")).rejects.toThrow();
    expect(await manager.call("signup_policy_set", { deploymentId: receipt.id, mode: "closed" })).toMatchObject({ mode: "closed", previousMode: "invitation" });

    const audit = new Database(path.join(root, "mcp.sqlite"));
    const rows = audit.query("SELECT tool, target, ok FROM audit WHERE tool LIKE 'app_%' AND target = ?1").all(receipt.id) as Array<{ tool: string; ok: number }>;
    audit.close();
    expect(new Set(rows.map((row) => row.tool))).toEqual(new Set(["app_invitations_create", "app_users_list", "app_invitations_list", "app_user_suspend"]));
   }, 60_000);

  test("anonymous visitors on a public app may only call actions declared anonymous", async () => {
    const { receipt } = await release("anonymous_proof");
    const listed = await fetch(`${url}/_littlebox/mcp`, { method: "POST", headers: { host: receipt.hostname, "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }) });
    expect(listed.status).toBe(200);
    expect((await listed.json()).result.tools.map((tool: { name: string }) => tool.name)).toEqual(["auth_session", "actions_invoke", "files_list", "files_read", "transfer_read", "transfer_status", "transfer_cancel", "auth_signup", "auth_verify", "auth_recover", "auth_reset"]);
    const visitor = runtimeClient(receipt.hostname);
    expect(await visitor.call("auth_session")).toEqual({ userId: "anonymous", name: "Anonymous", role: "anonymous" });
    expect(await visitor.call("actions_invoke", { name: "notes.count" })).toEqual([{ total: 0 }]);
    await expect(visitor.call("actions_invoke", { name: "notes.list" })).rejects.toThrow("authentication-required");
    await expect(visitor.call("actions_invoke", { name: "notes.create", input: { title: "Spam" } })).rejects.toThrow("authentication-required");
    expect(await visitor.call("files_list")).toEqual([]);
    await expect(visitor.call("files_read", { fileId: crypto.randomUUID() })).rejects.toThrow("File not found");
    await expect(visitor.call("files_upload", { transferId: crypto.randomUUID(), name: "x.txt" })).rejects.toThrow("authentication-required");
    await expect(visitor.call("transfer_begin", { size: 1, sha256: "a".repeat(64) })).rejects.toThrow("authentication-required");
    await expect(visitor.upload(new TextEncoder().encode("anonymous upload"))).rejects.toThrow("authentication-required");
    await expect(visitor.call("auth_logout")).rejects.toThrow("authentication-required");
    await expect(visitor.call("database_execute", { deploymentId: receipt.id, operation: "inspect" })).rejects.toThrow("Unknown tool");
    const user = await appUser(receipt.hostname, "member@app.test");
    await user.call("actions_invoke", { name: "notes.create", input: { title: "Counted" } });
    expect(await visitor.call("actions_invoke", { name: "notes.count" })).toEqual([{ total: 1 }]);
    const tooLarge = await fetch(`${url}/_littlebox/mcp`, { method: "POST", headers: { host: receipt.hostname }, body: "x".repeat(300_000) });
    expect(tooLarge.status).toBe(413);
    const crossOrigin = await fetch(`${url}/_littlebox/mcp`, { method: "POST", headers: { host: receipt.hostname, origin: "https://attacker.test" }, body: "{}" });
    expect(crossOrigin.status).toBe(403);
  });

  test("private apps start closed and refuse anonymous visitors", async () => {
    const { receipt } = await release("private_proof", "private");
    expect(await alice.call("signup_policy_read", { deploymentId: receipt.id })).toMatchObject({ mode: "closed", visibility: "private" });
    expect((await signup(receipt.hostname, "visitor@app.test")).status).toBe(403);
    expect(await loginPage(receipt.hostname)).not.toContain("/_littlebox/auth/signup");
    const anonymous = await fetch(`${url}/_littlebox/mcp`, { method: "POST", headers: { host: receipt.hostname, "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }) });
    expect(anonymous.status).toBe(401);
    await expect(runtimeClient(receipt.hostname).call("actions_invoke", { name: "notes.count" })).rejects.toThrow("Sign in");
    await alice.call("signup_policy_set", { deploymentId: receipt.id, mode: "allowlist", allowlist: ["visitor@app.test"] });
    const visitor = await appUser(receipt.hostname, "visitor@app.test");
    expect(await visitor.call("actions_invoke", { name: "notes.count" })).toEqual([{ total: 0 }]);
  });

  test("republishing a public app as private closes an open signup", async () => {
    const { receipt } = await release("visibility_flip_proof");
    expect(await alice.call("signup_policy_read", { deploymentId: receipt.id })).toMatchObject({ mode: "open", visibility: "public" });
    await release("visibility_flip_proof", "private");
    expect(await alice.call("signup_policy_read", { deploymentId: receipt.id })).toMatchObject({ mode: "closed", visibility: "private" });
    expect((await signup(receipt.hostname, "stranger@app.test")).status).toBe(403);
  });

  test("signup answers the same for closed and unlisted addresses", async () => {
    const { receipt } = await release("signup_oracle_proof");
    await alice.call("signup_policy_set", { deploymentId: receipt.id, mode: "allowlist", allowlist: ["listed@app.test"] });
    const attempt = async (email: string, pw: string) => {
      const response = await fetch(`${url}/_littlebox/auth/signup`, { method: "POST", headers: { host: receipt.hostname, "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email, password: pw }) });
      return { status: response.status, body: await response.json() };
    };
    const listedWeak = await attempt("listed@app.test", "x");
    expect(listedWeak.status).toBe(400);
    expect(await attempt("unlisted@app.test", "x")).toEqual(listedWeak);
    const unlisted = await attempt("unlisted@app.test", password);
    expect(unlisted.status).toBe(403);
    await alice.call("signup_policy_set", { deploymentId: receipt.id, mode: "closed" });
    expect(await attempt("listed@app.test", password)).toEqual(unlisted);
  });

  test("app user actions are rate limited and not audited", async () => {
    const { receipt } = await release("rate_limit_proof");
    const visitor = runtimeClient(receipt.hostname);
    for (let i = 0; i < 30; i++) expect(await visitor.call("actions_invoke", { name: "notes.count" })).toEqual([{ total: 0 }]);
    await expect(visitor.call("actions_invoke", { name: "notes.count" })).rejects.toThrow("rate-limited");
    // Signed-in users are not counted against the anonymous limit; each has a
    // bucket of 120 calls per 10 seconds.
    const member = await appUser(receipt.hostname, "member@limit.test");
    // The bucket refills while the loop runs, so count calls until refused.
    let accepted = 0;
    for (; accepted < 400; accepted++) {
      try { await member.call("actions_invoke", { name: "notes.count" }); }
      catch (e) { expect(String(e)).toContain("rate-limited"); break; }
    }
    expect(accepted).toBeGreaterThanOrEqual(120);
    expect(accepted).toBeLessThan(400);
    const other = await appUser(receipt.hostname, "other@limit.test");
    expect(await other.call("actions_invoke", { name: "notes.count" })).toEqual([{ total: 0 }]);
    // Only an upload that stores a new file is audited. Failures and receipt
    // replays leave no row, so a stranger cannot grow the audit table with them.
    const upload = await other.upload(new TextEncoder().encode("audited once"));
    const saved = await other.call("files_upload", { transferId: upload.transferId, name: "once.txt", type: "text/plain" });
    for (let i = 0; i < 3; i++) expect(await other.call("files_upload", { transferId: upload.transferId, name: "once.txt", type: "text/plain" })).toEqual(saved);
    await other.cancel(upload.transferId);
    await expect(other.call("files_upload", { transferId: crypto.randomUUID(), name: "missing.txt" })).rejects.toThrow();
    await expect(other.call("files_upload", { transferId: "not-a-uuid", name: "bad.txt" })).rejects.toThrow("Invalid transfer ID");
    const db = new Database(path.join(root, "mcp.sqlite"));
    try {
      expect(db.query("SELECT COUNT(*) AS n FROM audit WHERE actor LIKE 'runtime:%' AND tool <> 'files_upload'").get()).toEqual({ n: 0 });
      expect(db.query("SELECT COUNT(*) AS n, SUM(ok) AS ok FROM audit WHERE actor LIKE 'runtime:%' AND tool = 'files_upload' AND target = ?1").get(receipt.id)).toEqual({ n: 1, ok: 1 });
      const applied = db.query("SELECT target, detail FROM audit WHERE tool='release_apply' AND target='rate_limit_proof'").get() as { detail: string };
      expect(applied.detail).toBe(`deployment ${receipt.id}`);
      const changed = db.query("SELECT detail FROM audit WHERE tool='signup_policy_set' AND detail IS NOT NULL ORDER BY id LIMIT 1").get() as { detail: string };
      expect(changed.detail).toMatch(/^(open|allowlist|closed) to (open|allowlist|closed)$/);
    } finally { db.close(); }
  });

  test("a malformed transfer ID is refused before it takes a lock", async () => {
    const { receipt } = await release("transfer_id_proof");
    const user = await appUser(receipt.hostname, "uploader@app.test");
    await expect(user.call("files_upload", { transferId: `x-${"a".repeat(1000)}`, name: "f" })).rejects.toThrow("Invalid transfer ID");
    await expect(user.call("transfer_status", { transferId: "not-a-uuid" })).rejects.toThrow("Invalid transfer ID");
  });

  test("app and owner sign-in are rate limited per client and per email from that client", async () => {
    const { receipt } = await release("auth_limit_proof");
    const attempt = (route: string, client: string, email: string, host?: string) => fetch(`${url}${route}`, { method: "POST", headers: { ...(host ? { host } : {}), "content-type": "application/json", "x-forwarded-for": client }, body: JSON.stringify({ email, password: "wrong-password-123" }) });
    for (const [route, host] of [["/_littlebox/auth/login", receipt.hostname], ["/_littlebox/owner/login", undefined]] as const) {
      const client = freshClient();
      // Concurrent, so the test also shows that password checks run in parallel.
      expect((await Promise.all(Array.from({ length: 10 }, (_, i) => attempt(route, client, `guess${i}@limit.test`, host)))).map(r => r.status)).toEqual(Array(10).fill(401));
      const refused = await attempt(route, client, "another@limit.test", host);
      expect(refused.status).toBe(429);
      expect(Number(refused.headers.get("retry-after"))).toBeGreaterThan(0);
      const guesser = freshClient();
      expect((await Promise.all(Array.from({ length: 5 }, () => attempt(route, guesser, "target@limit.test", host)))).map(r => r.status)).toEqual(Array(5).fill(401));
      expect((await attempt(route, guesser, "Target@limit.test", host)).status).toBe(429);
      expect((await attempt(route, freshClient(), "Target@limit.test", host)).status).toBe(401);
    }
    // Signup counts against the same buckets.
    const client = freshClient();
    for (let i = 0; i < 10; i++) await fetch(`${url}/_littlebox/auth/signup`, { method: "POST", headers: { host: receipt.hostname, "content-type": "application/json", "x-forwarded-for": client }, body: JSON.stringify({ email: `new${i}@limit.test`, password: "x" }) });
    expect((await fetch(`${url}/_littlebox/auth/signup`, { method: "POST", headers: { host: receipt.hostname, "content-type": "application/json", "x-forwarded-for": client }, body: JSON.stringify({ email: "late@limit.test", password }) })).status).toBe(429);
  }, 60_000);

  test("failed guesses from one address do not lock the account holder out", async () => {
    const { receipt } = await release("lockout_proof");
    await appUser(receipt.hostname, "victim@lockout.test");
    for (const [route, host, email] of [["/_littlebox/auth/login", receipt.hostname, "victim@lockout.test"], ["/_littlebox/owner/login", undefined, "alice@owner.test"]] as const) {
      const send = (client: string, pw: string) => fetch(`${url}${route}`, { method: "POST", headers: { ...(host ? { host } : {}), "content-type": "application/json", "x-forwarded-for": client }, body: JSON.stringify({ email, password: pw }) });
      const attacker = freshClient();
      const statuses = [];
      for (let i = 0; i < 8; i++) statuses.push((await send(attacker, "wrong-password-123")).status);
      expect(statuses).toEqual([...Array(5).fill(401), ...Array(3).fill(429)]);
      expect((await send(freshClient(), password)).status).toBe(200);
    }
  }, 60_000);

  test("password hashing does not hold the identity lock", async () => {
    const { receipt } = await release("auth_lock_proof");
    const user = await appUser(receipt.hostname, "steady@app.test");
    const login = (email: string, pw: string) => fetch(`${url}/_littlebox/auth/login`, { method: "POST", headers: { host: receipt.hostname, "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email, password: pw }) });
    let started = performance.now();
    expect((await login("steady@app.test", password)).status).toBe(200);
    const oneHash = performance.now() - started;
    // Logout takes the identity lock but hashes nothing. Before the fix it
    // queued behind every password check in flight.
    const flood = Array.from({ length: 6 }, (_, i) => login(`flood${i}@app.test`, "wrong-password-123"));
    await Bun.sleep(20);
    started = performance.now();
    await user.call("auth_logout");
    const logout = performance.now() - started;
    expect((await Promise.all(flood)).map(r => r.status)).toEqual(Array(6).fill(401));
    expect(logout).toBeLessThan(oneHash / 2);
  }, 60_000);

  test("the signup policy survives a Kernel restart", async () => {
    const { receipt } = await release("restart_proof");
    await alice.call("signup_policy_set", { deploymentId: receipt.id, mode: "allowlist", allowlist: ["@kept.test"] });
    kernel.kill();
    await kernel.exited;
    await startKernel();
    expect(await alice.call("signup_policy_read", { deploymentId: receipt.id })).toMatchObject({ mode: "allowlist", allowlist: ["@kept.test"] });
    expect((await signup(receipt.hostname, "someone@other.test")).status).toBe(403);
    expect((await signup(receipt.hostname, "someone@kept.test")).status).toBe(200);
    // A redeploy keeps the owner's choice.
    await release("restart_proof");
    expect(await alice.call("signup_policy_read", { deploymentId: receipt.id })).toMatchObject({ mode: "allowlist", allowlist: ["@kept.test"] });
  }, 30_000);

  test("OAuth binds consent, resource, redirect, PKCE and one-time codes", async () => {
    const metadata = await (await fetch(`${url}/.well-known/oauth-authorization-server`)).json();
    expect(metadata.code_challenge_methods_supported).toEqual(["S256"]);
    const registration = await (await fetch(`${url}/oauth/register`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ client_name: "Integration agent", redirect_uris: ["http://127.0.0.1:9999/callback"] }) })).json();
    const verifier = "a".repeat(64);
    const challenge = Buffer.from(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))).toString("base64url");
    const args = { client_id: registration.client_id, redirect_uri: registration.redirect_uris[0], response_type: "code", code_challenge_method: "S256", code_challenge: challenge, scope: "read", resource: `${url}/mcp`, state: "test-state" };
    const consent = await fetch(`${url}/oauth/authorize?${new URLSearchParams(args)}`, { headers: { cookie: aliceCookie } });
    const html = await consent.text();
    const csrf = html.match(/name=csrf value="([^"]+)"/)![1];
    const approved = await fetch(`${url}/oauth/authorize`, { method: "POST", headers: { cookie: aliceCookie, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ ...args, csrf }), redirect: "manual" });
    expect(approved.status).toBe(303);
    const redirect = new URL(approved.headers.get("location")!);
    expect(redirect.searchParams.get("state")).toBe("test-state");
    const exchange = { grant_type: "authorization_code", client_id: args.client_id, redirect_uri: args.redirect_uri, code: redirect.searchParams.get("code")!, resource: args.resource, code_verifier: verifier };
    const redeem = (data: object) => fetch(`${url}/oauth/token`, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(data as Record<string, string>) });
    expect((await redeem({ ...exchange, code_verifier: "b".repeat(64) })).status).toBe(400);
    const token = await (await redeem(exchange)).json();
    expect(token.token_type).toBe("Bearer");
    expect((await redeem(exchange)).status).toBe(400);
    const client = new McpClient(`${url}/mcp`, { token: token.access_token });
    expect((await client.call<{ projects: unknown[] }>("projects_list")).projects.length).toBeGreaterThan(0);
    await expect(client.call("project_create", { name: "Escalate" })).rejects.toThrow("Unknown tool");
  });

  test("the official SDK discovers OAuth, registers, obtains consent and exchanges its PKCE code", async () => {
    let clientInformation: any;
    let tokens: any;
    let verifier = "";
    let authorizationUrl: URL | undefined;
    const provider: OAuthClientProvider = {
      redirectUrl: "http://127.0.0.1:9998/callback",
      clientMetadata: { client_name: "SDK OAuth test", redirect_uris: ["http://127.0.0.1:9998/callback"], token_endpoint_auth_method: "none", grant_types: ["authorization_code"], response_types: ["code"], scope: "read" },
      clientInformation: () => clientInformation,
      saveClientInformation: info => { clientInformation = info; },
      tokens: () => tokens,
      saveTokens: value => { tokens = value; },
      redirectToAuthorization: target => { authorizationUrl = target; },
      saveCodeVerifier: value => { verifier = value; },
      codeVerifier: () => verifier,
      state: () => "sdk-oauth-state",
    };
    const transport = new StreamableHTTPClientTransport(new URL(`${url}/mcp`), { authProvider: provider });
    const first = new Client({ name: "oauth-agent", version: "1.0.0" });
    await expect(first.connect(transport)).rejects.toThrow();
    expect(authorizationUrl).toBeDefined();
    const consent = await fetch(authorizationUrl!, { headers: { cookie: aliceCookie } });
    const html = await consent.text();
    const csrf = html.match(/name=csrf value="([^"]+)"/)![1];
    const args = Object.fromEntries(authorizationUrl!.searchParams);
    const approval = await fetch(`${url}/oauth/authorize`, { method: "POST", headers: { cookie: aliceCookie, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ ...args, csrf }), redirect: "manual" });
    expect(approval.status).toBe(303);
    const code = new URL(approval.headers.get("location")!).searchParams.get("code")!;
    await transport.finishAuth(code);
    const client = new Client({ name: "authorized-agent", version: "1.0.0" });
    try {
      await client.connect(new StreamableHTTPClientTransport(new URL(`${url}/mcp`), { authProvider: provider }));
      expect((await client.listTools()).tools.some(tool => tool.name === "projects_list")).toBe(true);
    } finally { await client.close(); await first.close(); }
  });

  // Keep this test last: the registration limit is per address and every test runs from 127.0.0.1.
  test("OAuth registration is rate limited per address", async () => {
    const register = () => fetch(`${url}/oauth/register`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ client_name: "Flood", redirect_uris: ["http://127.0.0.1:9997/callback"] }) });
    let refused: Response | undefined;
    for (let attempt = 1; attempt <= 12 && !refused; attempt++) {
      const response = await register();
      if (response.status === 429) refused = response;
      else expect(response.status).toBe(201);
    }
    expect(refused).toBeDefined();
    expect(refused!.headers.get("retry-after")).toBeTruthy();
    expect((await refused!.json()).error).toBe("rate-limited");
  });
});
