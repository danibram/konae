import { expect, test } from "bun:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { Database } from "bun:sqlite";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { initLocal } from "./konae";

const binary = process.env.KONAE_KERNEL_BIN;
const integration = binary ? test : test.skip;

async function freePort() {
  const server = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => new Response() });
  const port = server.port;
  server.stop(true);
  return port;
}

async function challenge(verifier: string) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return Buffer.from(digest).toString("base64url");
}

integration("app OAuth grants run MCP SDK calls as two isolated app members", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-app-mcp-oauth-"));
  let child: ReturnType<typeof Bun.spawn> | undefined;
  try {
    const settings = await initLocal(root, { name: "MCP OAuth integration" });
    settings.visibility = "public";
    settings.signupPolicy = { mode: "open", allowlist: [] };
    settings.modules = { ...settings.modules, actions: { on: true }, auth: { on: true }, mcp: { on: true } };
    await writeFile(path.join(root, "konae.json"), `${JSON.stringify(settings, null, 2)}\n`);
    await writeFile(path.join(root, "app/schema.sql"), "CREATE TABLE IF NOT EXISTS notes (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, body TEXT NOT NULL);\n");
    await writeFile(path.join(root, "app/actions.json"), `${JSON.stringify({ version: 1, actions: {
      "notes.mine": { kind: "query", access: "authenticated", statement: "SELECT body FROM notes WHERE user_id = ?1 LIMIT 20", bindings: [{ source: "actor", field: "id" }], mcp: { description: "List the signed-in member's notes." } },
      "notes.hidden": { kind: "query", access: "authenticated", statement: "SELECT body FROM notes LIMIT 20", bindings: [], mcp: { description: "Not granted to this client." } },
    } }, null, 2)}\n`);

    const port = await freePort();
    let eventsPort = await freePort();
    while (eventsPort === port) eventsPort = await freePort();
    child = Bun.spawn([process.execPath, path.resolve("scripts/konae.ts"), "dev", "--dir", root, "--port", String(port), "--events-port", String(eventsPort)], {
      env: { ...process.env, KONAE_KERNEL_BIN: binary! }, stdout: "pipe", stderr: "pipe",
    });
    let output = "";
    void (async () => { for await (const chunk of child!.stdout as ReadableStream<Uint8Array>) output += new TextDecoder().decode(chunk); })();
    void (async () => { for await (const chunk of child!.stderr as ReadableStream<Uint8Array>) output += new TextDecoder().decode(chunk); })();
    const appUrl = `http://${settings.slug}.localhost:${port}`;
    for (let attempt = 0; attempt < 300; attempt++) {
      if (child.exitCode !== null) throw Error(`Local dev exited early. ${output}`);
      if (await fetch(`${appUrl}/_konae/mcp`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "probe", version: "1" } } }) }).then((response) => response.status !== 404, () => false)) break;
      await Bun.sleep(100);
    }

    const clientsPath = path.join(root, ".konae/kernel/mcp-app-clients.json");
    await mkdir(path.dirname(clientsPath), { recursive: true });
    await writeFile(clientsPath, `${JSON.stringify({ version: 1, clients: [{ clientId: "desktop-notes", appId: settings.id, name: "Notes desktop client", redirectUris: ["http://127.0.0.1:8765/callback"] }] }, null, 2)}\n`);
    const metadata = await fetch(`${appUrl}/.well-known/oauth-protected-resource/_konae/mcp`);
    expect(metadata.status).toBe(200);
    const discovered = await metadata.json() as { resource: string; scopes_supported: string[] };
    expect(discovered.resource).toBe(`${appUrl}/_konae/mcp`);
    expect(discovered.scopes_supported.sort()).toEqual(["notes.hidden", "notes.mine"]);

    const signUp = async (email: string) => {
      const response = await fetch(`${appUrl}/_konae/auth/signup`, { method: "POST", headers: { "content-type": "application/json", origin: appUrl }, body: JSON.stringify({ email, password: "a long integration password" }) });
      expect(response.status, await response.clone().text()).toBe(200);
      return email;
    };
    const grant = async (email: string) => {
      const verifier = crypto.randomUUID().replaceAll("-", "") + crypto.randomUUID().replaceAll("-", "");
      const state = crypto.randomUUID();
      const resource = `${appUrl}/_konae/mcp`;
      const params = new URLSearchParams({ response_type: "code", client_id: "desktop-notes", redirect_uri: "http://127.0.0.1:8765/callback", state, code_challenge: await challenge(verifier), code_challenge_method: "S256", resource, scope: "notes.mine" });
      const authorize = `${resource}/oauth/authorize?${params}`;
      const signInPage = await fetch(authorize);
      expect(signInPage.status).toBe(200);
      expect(await signInPage.text()).toContain("Sign in with your app account");
      const signedIn = await fetch(`${resource}/oauth/signin`, { method: "POST", headers: { origin: appUrl, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ ...Object.fromEntries(params), email, password: "a long integration password" }), redirect: "manual" });
      expect(signedIn.status).toBe(303);
      const cookie = signedIn.headers.get("set-cookie")?.split(";", 1)[0];
      expect(cookie).toBeTruthy();
      const consent = await fetch(signedIn.headers.get("location")!, { headers: { cookie: cookie! } });
      expect(consent.status).toBe(200);
      expect(await consent.text()).toContain("Allow access");
      const approved = await fetch(`${resource}/oauth/authorize`, { method: "POST", headers: { cookie: cookie!, origin: appUrl, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ ...Object.fromEntries(params), decision: "allow" }), redirect: "manual" });
      expect(approved.status).toBe(303);
      const callback = new URL(approved.headers.get("location")!);
      expect(callback.searchParams.get("state")).toBe(state);
      const exchanged = await fetch(`${resource}/oauth/token`, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ grant_type: "authorization_code", client_id: "desktop-notes", code: callback.searchParams.get("code")!, redirect_uri: "http://127.0.0.1:8765/callback", code_verifier: verifier, resource }) });
      expect(exchanged.status, await exchanged.clone().text()).toBe(200);
      return (await exchanged.json() as { access_token: string }).access_token;
    };

    const [cookieA, cookieB] = await Promise.all([signUp("member-a@example.test"), signUp("member-b@example.test")]);
    const [tokenA, tokenB] = await Promise.all([grant(cookieA), grant(cookieB)]);
    const identity = new Database(path.join(root, "data/identity.sqlite"), { readonly: true });
    const users = identity.query("SELECT id, email FROM app_users ORDER BY email").all() as Array<{ id: string; email: string }>;
    identity.close();
    expect(users).toHaveLength(2);
    const database = new Database(path.join(root, "data/database.sqlite"));
    database.query("INSERT INTO notes (id,user_id,body) VALUES (?1,?2,?3)").run("a-note", users[0]!.id, "member A note");
    database.query("INSERT INTO notes (id,user_id,body) VALUES (?1,?2,?3)").run("b-note", users[1]!.id, "member B note");
    database.close();

    const endpoint = new URL(`${appUrl}/_konae/mcp`);
    const connect = async (token: string) => {
      const client = new Client({ name: "mcp-oauth-test", version: "1.0.0" });
      const transport = new StreamableHTTPClientTransport(endpoint, { requestInit: { headers: { authorization: `Bearer ${token}` } } });
      await client.connect(transport);
      return { client, transport };
    };
    const memberA = await connect(tokenA);
    const memberB = await connect(tokenB);
    const listed = await memberA.client.listTools();
    expect(listed.tools.map((tool) => tool.name)).toEqual(["notes.mine"]);
    const answerA = await memberA.client.callTool({ name: "notes.mine", arguments: {} });
    const answerB = await memberB.client.callTool({ name: "notes.mine", arguments: {} });
    expect(JSON.stringify(answerA)).toContain("member A note");
    expect(JSON.stringify(answerA)).not.toContain("member B note");
    expect(JSON.stringify(answerB)).toContain("member B note");
    expect(JSON.stringify(answerB)).not.toContain("member A note");
    await expect(memberA.client.callTool({ name: "notes.hidden", arguments: {} })).rejects.toThrow("Unknown tool or insufficient permission");

    const crossApp = await fetch(`http://other-app.localhost:${port}/_konae/mcp`, { method: "POST", headers: { authorization: `Bearer ${tokenA}`, "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 9, method: "tools/list" }) });
    expect(crossApp.status).not.toBe(200);

    const resource = `${appUrl}/_konae/mcp`;
    const revoked = await fetch(`${resource}/oauth/revoke`, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ client_id: "desktop-notes", token: tokenA }) });
    expect(revoked.status).toBe(200);
    const afterRevoke = await fetch(endpoint, { method: "POST", headers: { authorization: `Bearer ${tokenA}`, "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 10, method: "tools/list" }) });
    expect(afterRevoke.status).toBe(401);
    const identityWrite = new Database(path.join(root, "data/identity.sqlite"));
    identityWrite.query("UPDATE app_users SET state='suspended' WHERE id=?1").run(users[1]!.id);
    identityWrite.close();
    const afterSuspend = await fetch(endpoint, { method: "POST", headers: { authorization: `Bearer ${tokenB}`, "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 11, method: "tools/list" }) });
    expect(afterSuspend.status).toBe(401);
    await memberA.client.close();
    await memberB.client.close();
  } catch (error) {
    throw Error(`${error instanceof Error ? error.message : error}\n${error instanceof Error ? error.stack : ""}`);
  } finally {
    if (child && child.exitCode === null) { child.kill("SIGTERM"); await child.exited; }
    await rm(root, { recursive: true, force: true });
  }
}, 180_000);
