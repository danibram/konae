import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { templateChoices } from "../src/lib/templates";
import { PROJECT_TEMPLATES_FILE, renderProjectTemplates } from "./project-templates";
import { deploymentRecord, formatEvent, kernelEvents, mailEvents, serveEvents, type ReadRecord } from "./events";
import { usageRange } from "../src/lib/usage";
import type { Sources } from "../src/lib/what-happened";
import { DOMAIN_NEEDS_DEPLOYMENT, HELP, INIT_TEMPLATES, NO_INSTALLATION, SERVER_COMMANDS, TEST_ACCOUNT_EMAIL, VERIFY_BY_LINK, browserLogin, creatorVerificationToken, domainLocal, hostnameLocal, ignoreInGit, initLocal, inviteTestAccount, invitationLink, isLoopbackUrl, kernelRecord, localKernel, loginCredentials, loginScopes, manifestSignupPolicy, pipedSecret, publishLocal, readSecret, serverCommandMessage, setSecret, signupPolicyWarning, toolArguments, type PromptInput } from "./konae";

/** A stand-in Kernel MCP endpoint that records tool calls and can fail chosen ones. */
const calls: Array<{ name: string; arguments: Record<string, unknown> }> = [];
const failing = new Set<string>();
/** Answers a test sets for one tool, over the defaults below. */
const answers: Record<string, unknown> = {};
let uploaded = "";
let server: ReturnType<typeof Bun.serve>;

beforeAll(() => {
  server = Bun.serve({
    port: 0,
    hostname: "127.0.0.1",
    async fetch(request) {
      const message = await request.json() as { id?: string; method: string; params?: { name: string; arguments: Record<string, unknown> } };
      if (message.id === undefined) return new Response(null, { status: 202 });
      const reply = (result: unknown) => Response.json({ jsonrpc: "2.0", id: message.id, result });
      if (message.method === "initialize") return reply({ protocolVersion: "2025-11-25" });
      const { name, arguments: args } = message.params!;
      calls.push({ name, arguments: args });
      if (failing.has(name)) return reply({ isError: true, content: [{ type: "text", text: `${name} failed` }] });
      if (name === "transfer_write") uploaded += atob(String(args.data));
      const results: Record<string, unknown> = {
        project_create: { id: "app_test", name: "Test", slug: "test", manifest: {}, files: { "app.js": "" } },
        transfer_begin: { transferId: "transfer_1", size: 0, sha256: "", chunkBytes: 65_536 },
        release_plan: { planId: "plan_1", schema: { statements: [], losses: [], confirmation: null } },
        release_apply: { id: "dep_1", url: "http://test.localhost/" },
      };
      return reply({ isError: false, structuredContent: { result: answers[name] ?? results[name] ?? {} } });
    },
  });
});

afterAll(() => server.stop(true));

let directory: string;
beforeEach(async () => {
  calls.length = 0;
  failing.clear();
  for (const name of Object.keys(answers)) delete answers[name];
  uploaded = "";
  directory = await mkdtemp(path.join(tmpdir(), "konae-cli-"));
  await mkdir(path.join(directory, "app"));
});
afterEach(async () => { await rm(directory, { recursive: true, force: true }); });

const origin = () => `http://127.0.0.1:${server.port}`;

function cli(...args: string[]) {
  return Bun.spawn([process.execPath, "run", path.join(import.meta.dir, "konae.ts"), ...args], {
    env: { ...process.env, KONAE_MCP_TOKEN: "token" },
    stdout: "pipe",
    stderr: "pipe",
  });
}

const rootPackage = JSON.parse(await readFile(path.join(import.meta.dir, "../package.json"), "utf8")) as { version: string };

test("the version init writes is the one package.json and kernel-rs/Cargo.toml name", async () => {
  const cargo = await readFile(path.join(import.meta.dir, "../kernel-rs/Cargo.toml"), "utf8");
  const workspace = cargo.slice(cargo.indexOf("[workspace.package]"));
  expect(workspace.match(/^version = "(.*)"$/m)?.[1]).toBe(rootPackage.version);
});

test("a folder init wrote builds without npm install, with the built-in bundler", async () => {
  const folder = await mkdtemp(path.join(tmpdir(), "konae-fresh-"));
  try {
    const run = (...args: string[]) => Bun.spawn([process.execPath, "run", path.join(import.meta.dir, "konae.ts"), ...args], {
      cwd: folder, env: Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))) as Record<string, string>, stdout: "pipe", stderr: "pipe",
    });
    const init = run("init", "--dir", folder, "--name", "Fresh");
    expect(await init.exited).toBe(0);
    expect(await stat(path.join(folder, "node_modules")).then(() => true, () => false)).toBe(false);
    const built = run("build", "--dir", folder);
    const [status, stderr] = await Promise.all([built.exited, new Response(built.stderr).text()]);
    expect(stderr).not.toContain("npm run build failed");
    expect(status).toBe(0);
    expect(await readFile(path.join(folder, "dist/index.html"), "utf8")).toContain("<");
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
}, 60_000);

describe("konae CLI", () => {
  test.each(["release_plan", "release_apply"])("a failed %s releases its upload so later publishes can proceed", async (step) => {
    await writeFile(path.join(directory, "app", "app.js"), "console.log('hi');");
    failing.add(step);
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest: {} } };

    for (let attempt = 0; attempt < 9; attempt++) {
      await expect(publishLocal(directory, config, "token")).rejects.toThrow(`${step} failed`);
    }
    expect(calls.filter((call) => call.name === "transfer_cancel")).toHaveLength(9);
    failing.clear();
    expect((await publishLocal(directory, config, "token")).url).toBe("http://test.localhost/");
  });

  test("cleanup failure preserves the publishing error", async () => {
    await writeFile(path.join(directory, "app", "app.js"), "console.log('hi');");
    failing.add("release_plan");
    failing.add("transfer_cancel");
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest: {} } };

    await expect(publishLocal(directory, config, "token")).rejects.toThrow("release_plan failed");
  });

  test("a publish whose upload cannot be released afterwards still reports the release", async () => {
    await writeFile(path.join(directory, "app", "app.js"), "console.log('hi');");
    await writeFile(path.join(directory, "app", "index.html"), "<main id=app></main>");
    failing.add("transfer_cancel");
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest: {} } };

    const receipt = await publishLocal(directory, config, "token");

    expect(receipt.url).toBe("http://test.localhost/");
    expect(calls.map((call) => call.name)).toContain("transfer_cancel");
    // No visibility in the manifest publishes private, the same default as the exported deploy script.
    expect(JSON.parse(uploaded).visibility).toBe("private");
    const html = await readFile(path.join(directory, "dist/index.html"), "utf8");
    expect(html).toContain("<script data-konae-runtime>");
  });

  test("login adds manage_users and export_data only when a flag asks", () => {
    expect(loginScopes([])).toEqual(["read", "write", "publish"]);
    expect(loginScopes(["--manage-users"])).toEqual(["read", "write", "publish", "manage_users"]);
    expect(loginScopes(["--export-data", "--manage-users"])).toEqual(["read", "write", "publish", "manage_users", "export_data"]);
  });

  test("login --browser checks state, exchanges the code with its PKCE verifier, and gives up after the timeout", async () => {
    let challenge = ""; let exchanged: URLSearchParams | undefined;
    const issuer = Bun.serve({
      port: 0,
      async fetch(request) {
        const url = new URL(request.url); const origin = url.origin;
        if (url.pathname === "/.well-known/oauth-protected-resource") return Response.json({ resource: `${origin}/mcp`, authorization_servers: [origin] });
        if (url.pathname === "/.well-known/oauth-authorization-server") return Response.json({ issuer: origin, authorization_endpoint: `${origin}/oauth/authorize`, token_endpoint: `${origin}/oauth/token`, registration_endpoint: `${origin}/oauth/register` });
        if (url.pathname === "/oauth/register") return Response.json({ client_id: "cli", ...(await request.json() as object) }, { status: 201 });
        if (url.pathname === "/oauth/token") { exchanged = new URLSearchParams(await request.text()); return Response.json({ access_token: "lb_token", expires_in: 86400 }); }
        return new Response("", { status: 404 });
      },
    });
    const origin = `http://127.0.0.1:${issuer.port}`;
    const lines: string[] = [];
    try {
      // The browser: a stray answer with another state first, which login ignores, then the owner's.
      const signedIn = await browserLogin(origin, ["read", "write", "publish", "export_data"], {
        print: (line) => lines.push(line),
        openUrl: (address) => void (async () => {
          const authorize = new URL(address);
          challenge = authorize.searchParams.get("code_challenge")!;
          const callback = new URL(authorize.searchParams.get("redirect_uri")!);
          callback.searchParams.set("code", "the-code"); callback.searchParams.set("state", "forged");
          expect((await fetch(callback)).status).toBe(400);
          callback.searchParams.set("state", authorize.searchParams.get("state")!); callback.searchParams.set("iss", origin);
          expect((await fetch(callback)).status).toBe(200);
        })(),
      });
      expect(signedIn).toEqual({ token: "lb_token", expiresIn: 86400 });
      expect(exchanged?.get("code")).toBe("the-code");
      expect(exchanged?.get("resource")).toBe(`${origin}/mcp`);
      const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(exchanged!.get("code_verifier")!)));
      expect(Buffer.from(digest).toString("base64url")).toBe(challenge);
      expect(lines.join("\n")).toContain(`${origin}/oauth/authorize?`);
      expect(lines.join("\n")).toContain("scope=read+write+publish+export_data");
      expect(lines.join("\n")).not.toContain("the-code");

      await expect(browserLogin(origin, ["read"], { print: () => {}, open: false, timeoutMs: 50 })).rejects.toThrow("Nobody finished the sign-in within");
    } finally { issuer.stop(true); }
    expect(HELP).toContain("--browser");
    expect(NO_INSTALLATION).toContain("--url");
    expect(NO_INSTALLATION).toContain("login --browser");
  });

  test("secret set takes the value from stdin without its last line break and reads the name back", async () => {
    expect(pipedSecret("abc\n")).toBe("abc");
    expect(pipedSecret("abc\r\n")).toBe("abc");
    expect(pipedSecret("abc\n\n")).toBe("abc\n");
    const made: Array<[string, unknown]> = [];
    const listed = { secrets: [{ name: "google_client_secret", use: "google", updatedAt: 0 }] };
    const client = { call: async (name: string, input: unknown) => { made.push([name, input]); return (name === "secrets_list" ? listed : {}) as never; } };
    const line = await setSecret(client, "p1", "google_client_secret", "a-value-of-16-chars", "google");
    expect(made).toEqual([
      ["secret_set", { projectId: "p1", name: "google_client_secret", value: "a-value-of-16-chars", use: "google" }],
      ["secrets_list", { projectId: "p1" }],
    ]);
    expect(line).toContain("google_client_secret on p1 for use google");
    expect(line).not.toContain("a-value-of-16-chars");
    await expect(setSecret(client, "p1", "other", "a-value-of-16-chars")).rejects.toThrow("secrets_list does not list other");
    expect(made[2]).toEqual(["secret_set", { projectId: "p1", name: "other", value: "a-value-of-16-chars" }]);
  });

  test("login --verify-from reads the newest creator verification link for the email and works only on this machine", async () => {
    const line = (to: string, scope: string, purpose: string, token: string) => JSON.stringify({ to, purpose, scope, link: `http://127.0.0.1:4173/_konae/owner/login#${purpose}=${token}`, expiresAt: 1 });
    const mailbox = [
      line("ada@example.test", "creator", "verification", "old"),
      line("ada@example.test", "deployment:app", "verification", "app-account"),
      line("bob@example.test", "creator", "verification", "bob"),
      line("ada@example.test", "creator", "recovery", "recovery"),
      "not json",
      line("Ada@Example.test", "creator", "verification", "new"),
    ].join("\n");
    expect(creatorVerificationToken(mailbox, "ada@example.test")).toBe("new");
    expect(creatorVerificationToken(mailbox, "ada@example.test", 6)).toBeUndefined();
    expect(creatorVerificationToken(mailbox, "cara@example.test")).toBeUndefined();
    for (const url of ["http://localhost:4173", "http://127.0.0.1:19400", "http://[::1]:8787", "http://kernel.localhost"]) expect(isLoopbackUrl(url)).toBe(true);
    for (const url of ["https://konae.example.com", "http://192.168.1.4:4173", "http://localhost.example.com"]) expect(isLoopbackUrl(url)).toBe(false);
    expect(HELP).toContain("--verify-from <mailbox.jsonl>");
    expect(VERIFY_BY_LINK).toContain("--verify-from");
    const run = Bun.spawn([process.execPath, "run", "scripts/konae.ts", "login", "--dir", tmpdir(), "--url", "https://konae.example.com", "--verify-from", "/tmp/mailbox.jsonl"], {
      env: { ...process.env, KONAE_EMAIL: "ada@example.test", KONAE_PASSWORD: "a-long-password" }, stderr: "pipe", stdout: "ignore",
    });
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain("--verify-from reads a development mailbox on this machine, so it works only with a Kernel on localhost or 127.0.0.1, not konae.example.com.");
  });

  test("login without KONAE_PASSWORD or KONAE_EMAIL and without a terminal refuses as before", async () => {
    const refusal = "Set KONAE_EMAIL and KONAE_PASSWORD, or run login --browser to sign in through the browser without them. To create the account, also set KONAE_INVITATION_CODE, or KONAE_SETUP_KEY for the first owner.";
    await expect(loginCredentials({ KONAE_EMAIL: "ada@example.test" }, { isTTY: false })).rejects.toThrow(refusal);
    await expect(loginCredentials({ KONAE_PASSWORD: "a-long-password" }, { isTTY: undefined })).rejects.toThrow(refusal);
    const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))) as Record<string, string>;
    const run = Bun.spawn([process.execPath, "run", "scripts/konae.ts", "login", "--dir", tmpdir(), "--url", origin()], {
      env: { ...environment, KONAE_EMAIL: "ada@example.test" }, stdin: "ignore", stderr: "pipe", stdout: "ignore",
    });
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain(refusal);
  });

  test("login asks on a terminal only for what the environment leaves unset", async () => {
    const asked: Array<[string, boolean]> = [];
    const answer = async (question: string, secret: boolean) => { asked.push([question, secret]); return secret ? "a-long-password" : " ada@example.test "; };
    expect(await loginCredentials({}, { isTTY: true }, answer)).toEqual({ email: "ada@example.test", password: "a-long-password" });
    expect(asked).toEqual([["Email: ", false], ["Password for ada@example.test: ", true]]);
    asked.length = 0;
    expect(await loginCredentials({ KONAE_EMAIL: "bob@example.test" }, { isTTY: true }, answer)).toEqual({ email: "bob@example.test", password: "a-long-password" });
    expect(asked).toEqual([["Password for bob@example.test: ", true]]);
    asked.length = 0;
    expect(await loginCredentials({ KONAE_EMAIL: "bob@example.test", KONAE_PASSWORD: "set-in-env" }, { isTTY: true }, answer)).toEqual({ email: "bob@example.test", password: "set-in-env" });
    expect(asked).toEqual([]);
  });

  test("the password prompt does not echo, edits with backspace, restores the terminal and rejects on Ctrl-C", async () => {
    const stream = () => {
      let listener: ((chunk: string) => void) | undefined;
      const state = { raw: [] as boolean[], paused: 0, output: "" };
      const input: PromptInput = {
        setRawMode: (on) => { state.raw.push(on); }, resume: () => {}, pause: () => { state.paused++; },
        on: (_, next) => { listener = next; }, off: () => { listener = undefined; },
      };
      return { state, input, type: (text: string) => listener?.(text), listening: () => listener !== undefined, output: { write: (text: string) => { state.output += text; } } };
    };
    const typed = stream();
    const secret = readSecret("Password for ada@example.test: ", typed.input, typed.output);
    typed.type("a-lon");
    typed.type("x\u007fg-password\r");
    expect(await secret).toBe("a-long-password");
    expect(typed.state.output).toBe("Password for ada@example.test: \n");
    expect(typed.state.raw).toEqual([true, false]);
    expect(typed.state.paused).toBe(1);
    expect(typed.listening()).toBe(false);

    const cancelled = stream();
    const refused = readSecret("Password: ", cancelled.input, cancelled.output);
    cancelled.type("abc\u0003");
    await expect(refused).rejects.toThrow("Cancelled.");
    expect(cancelled.state.raw).toEqual([true, false]);
    expect(cancelled.listening()).toBe(false);
  });

  test("help says login asks for what is unset, and that dev invites the test account and keeps the Kernel's file limit", () => {
    const flat = HELP.replace(/\s+/g, " ");
    expect(flat).toContain("asks for either one that is unset when there is a terminal");
    expect(flat).toContain(`invited at ${TEST_ACCOUNT_EMAIL}`);
    expect(flat).toContain("KONAE_FILE_MAX_BYTES raises that limit");
  });

  test("the invitation link is the newest line for the address, past the lines already seen", () => {
    const line = (to: string, scope: string, purpose: string, token: string) => JSON.stringify({ to, purpose, scope, link: `http://localhost:8796/_konae/account#${purpose}=${token}`, expiresAt: 1 });
    const mailbox = [
      line("test@example.com", "deployment:app", "invitation", "old"),
      line("other@example.com", "deployment:app", "invitation", "other"),
      line("test@example.com", "creator", "invitation", "owner"),
      line("test@example.com", "deployment:app", "recovery", "recovery"),
      "not json",
      line("Test@Example.com", "deployment:app", "invitation", "new"),
    ].join("\n");
    expect(invitationLink(mailbox, "test@example.com")).toBe("http://localhost:8796/_konae/account#invitation=new");
    expect(invitationLink(mailbox, "test@example.com", 6)).toBeUndefined();
    expect(invitationLink(mailbox, "cara@example.com")).toBeUndefined();
  });

  test("dev invites the test account once, waits for its link, and says nothing new when accounts exist", async () => {
    const mailbox = path.join(directory, "mailbox.jsonl");
    const link = "http://localhost:8796/_konae/account#invitation=abc";
    const stub = (users: unknown[], afterInvite?: () => Promise<void>) => {
      const ran: string[][] = [];
      return { ran, kernel: { async run(args: string[]) {
        ran.push(args);
        if (args[0] === "users") return JSON.stringify({ users, nextCursor: null });
        await afterInvite?.();
        return "{}";
      } } };
    };
    const empty = stub([], async () => { await writeFile(mailbox, `${JSON.stringify({ to: TEST_ACCOUNT_EMAIL, purpose: "invitation", scope: "deployment:app", link, expiresAt: 1 })}\n`); });
    expect(await inviteTestAccount(empty.kernel, mailbox, 2)).toEqual({ line: `Test account: open ${link} and choose a password.` });
    expect(empty.ran).toEqual([["users"], ["invite", TEST_ACCOUNT_EMAIL]]);

    const held = stub([{ userId: "u1", email: "ada@example.test", state: "pending" }]);
    expect(await inviteTestAccount(held.kernel, mailbox, 2)).toEqual({});
    expect(held.ran).toEqual([["users"]]);

    const silent = stub([]);
    const lost = await inviteTestAccount(silent.kernel, path.join(directory, "none.jsonl"), 0.3);
    expect(lost.line).toBeUndefined();
    expect(lost.failed).toContain("did not reach");
    const refused = await inviteTestAccount({ async run(args) { if (args[0] === "users") return JSON.stringify({ users: [] }); throw Error("This app is closed."); } }, mailbox, 1);
    expect(refused).toEqual({ failed: "This app is closed." });
  });

  test("a local Kernel keeps the Kernel's own per-file limit, a 1 TiB quota, and takes either from the environment", async () => {
    const saved = { quota: process.env.KONAE_FILES_QUOTA_BYTES, file: process.env.KONAE_FILE_MAX_BYTES, bin: process.env.KONAE_KERNEL_BIN };
    const fake = path.join(directory, "kernel");
    await writeFile(fake, "");
    try {
      process.env.KONAE_KERNEL_BIN = fake;
      delete process.env.KONAE_FILES_QUOTA_BYTES; delete process.env.KONAE_FILE_MAX_BYTES;
      const settings = await initLocal(path.join(directory, "app-folder"), { name: "Probe" });
      const bare = await localKernel(path.join(directory, "app-folder"), settings);
      expect(bare.env.KONAE_FILES_QUOTA_BYTES).toBe(String(1024 ** 4));
      expect("KONAE_FILE_MAX_BYTES" in bare.env).toBe(false);
      process.env.KONAE_FILE_MAX_BYTES = "5242880"; process.env.KONAE_FILES_QUOTA_BYTES = "10485760";
      const raised = await localKernel(path.join(directory, "app-folder"), settings);
      expect([raised.env.KONAE_FILE_MAX_BYTES, raised.env.KONAE_FILES_QUOTA_BYTES]).toEqual(["5242880", "10485760"]);
    } finally {
      for (const [name, value] of [["KONAE_FILES_QUOTA_BYTES", saved.quota], ["KONAE_FILE_MAX_BYTES", saved.file], ["KONAE_KERNEL_BIN", saved.bin]] as const) {
        if (value === undefined) delete process.env[name]; else process.env[name] = value;
      }
    }
  });

  test("help names every command and every login flag, and an unknown command prints it", async () => {
    for (const word of ["login", "init", "dev", "build", "contract", "publish", "conform", "export", "--manage-users", "--export-data", "--verify-from", "--with-data", "--secret", "--key", "--inputs", "domain", "--deployment", "--set", "--clear", "pull", "sql", "events", "--prod", "--follow", "--json", "--events-port", "KONAE_KERNEL_BIN", "--file", "--reset", "--template", "tool", "--browser", "--no-open", "secret", "--use", "push", "hostname"]) expect(HELP).toContain(word);
    const run = cli("deploy");
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain("Unknown command deploy.\n\nUsage: bun run konae --");
  });

  test("help calls the address of an installation its installation URL and says what to put there", () => {
    expect(HELP).not.toContain("Kernel URL");
    expect(HELP).toContain("--url <installation URL>");
    expect(HELP).toContain("http://localhost:4173 under bun run dev:local");
    expect(HELP).toContain("http://app.localhost:8080 under Compose");
    expect(NO_INSTALLATION).not.toContain("Kernel URL");
    expect(Math.max(...HELP.split("\n").map((line) => line.length))).toBeLessThanOrEqual(125);
  });

  test("an operator command of the server binary is named as one, with where to run it", async () => {
    for (const name of ["owner-code-create", "owner-code-list", "owner-code-revoke", "owner-account-suspend", "owner-account-resume", "restore", "reconcile", "files-migrate"]) expect(SERVER_COMMANDS).toContain(name);
    expect(serverCommandMessage("owner-code-create")).toBe("owner-code-create is a command of the konae server binary, not of this CLI. Run KONAE_DATA_DIR=data/kernel-local kernel-rs/target/debug/konae owner-code-create for bun run dev:local, or docker compose exec --user konae kernel konae owner-code-create under Compose.");
    const run = cli("owner-code-create");
    expect(await run.exited).toBe(1);
    const said = await new Response(run.stderr).text();
    expect(said).toContain("owner-code-create is a command of the konae server binary, not of this CLI.");
    expect(said).toContain("docker compose exec --user konae kernel konae owner-code-create");
    expect(said).not.toContain("Unknown command");
    expect(said).not.toContain("Usage: bun run konae");
  });

  test("domain takes the Deployment from the id in konae.json, and refuses naming both ways when neither is given", async () => {
    answers.domain_read = { status: "none", generatedUrl: "http://notes.localhost" };
    const withUrl = (...args: string[]) => Bun.spawn([process.execPath, "run", path.join(import.meta.dir, "konae.ts"), "domain", "--url", origin(), ...args], {
      env: { ...process.env, KONAE_MCP_TOKEN: "token" }, stdout: "pipe", stderr: "pipe", cwd: tmpdir(),
    });
    await writeFile(path.join(directory, "konae.json"), JSON.stringify({ id: "app_from_folder", name: "Notes", slug: "notes" }));
    const fromFolder = withUrl("--dir", directory);
    expect(await fromFolder.exited).toBe(0);
    expect(await new Response(fromFolder.stdout).text()).toContain("This app has no custom domain.");
    expect(calls.map((call) => [call.name, call.arguments.deploymentId])).toEqual([["domain_read", "app_from_folder"]]);
    calls.length = 0;
    const named = withUrl("--dir", directory, "--deployment", "dep_named");
    expect(await named.exited).toBe(0);
    expect(calls.map((call) => call.arguments.deploymentId)).toEqual(["dep_named"]);
    calls.length = 0;
    const neither = withUrl();
    expect(await neither.exited).toBe(1);
    const refusal = await new Response(neither.stderr).text();
    expect(refusal).toContain(DOMAIN_NEEDS_DEPLOYMENT);
    expect(refusal).toContain("--deployment <id>");
    expect(refusal).toContain("--dir <folder>");
    expect(calls).toEqual([]);
    expect(HELP).toContain("[--deployment <id>]");
  });

  test("domain checks the hostname by the Kernel's rules before domain_set and prints the TXT record", async () => {
    const calls: string[] = [];
    const rules = { suffix: "apps.example.com", installationHosts: ["konae.example.com"] };
    let state: Record<string, unknown> = { hostname: null, status: "none", generatedUrl: "https://notes-1a2b3c.apps.example.com", rules };
    const client = {
      async call<T>(name: string, input: Record<string, unknown>): Promise<T> {
        calls.push(`${name} ${input.hostname ?? ""}`.trim());
        if (name === "domain_set") {
          state = { ...state, hostname: input.hostname, status: calls.filter((call) => call.startsWith("domain_set")).length > 1 ? "active" : "pending",
            url: `https://${input.hostname}`, dns: "No TXT record at _konae.notes.example.org yet.",
            record: { type: "TXT", name: `_konae.${input.hostname}`, value: "konae-verify-abc" } };
        }
        if (name === "domain_clear") return { cleared: state.hostname, generatedUrl: state.generatedUrl } as T;
        return state as T;
      },
    };
    await expect(domainLocal(client, "d1", { set: "shop.apps.example.com" })).rejects.toThrow(
      "shop.apps.example.com is under this installation's app suffix apps.example.com, where Konae names apps itself. Move the app's generated hostname there with hostname_set, or name a hostname outside the suffix.");
    expect(calls).toEqual(["domain_read"]);
    expect(await domainLocal(client, "d1", { set: "Notes.Example.org" })).toEqual([
      "notes.example.org does not serve yet. No TXT record at _konae.notes.example.org yet.",
      'Add this record at your DNS provider: TXT _konae.notes.example.org "konae-verify-abc"',
      "Point notes.example.org at this installation's proxy, then run this command again with --set notes.example.org.",
    ]);
    expect(await domainLocal(client, "d1", { set: "notes.example.org" })).toEqual([
      "https://notes.example.org serves this app, next to https://notes-1a2b3c.apps.example.com. Your proxy must terminate TLS for notes.example.org and forward Host unchanged.",
    ]);
    expect(await domainLocal(client, "d1", { clear: true })).toEqual(["notes.example.org no longer serves this app. https://notes-1a2b3c.apps.example.com keeps answering."]);
  });

  test("hostname shows the generated address, moves it, and passes the Kernel's refusal through unchanged", async () => {
    const refusal = "other.apps.example.com is another app's address on this installation. Choose another name.";
    const client = {
      async call<T>(name: string, input: Record<string, unknown>): Promise<T> {
        if (name === "domain_read") return { generatedUrl: "https://notes-1a2b3c.apps.example.com" } as T;
        if (input.name === "other") throw Error(refusal);
        return { url: `https://${input.name}.apps.example.com`, changed: true, previousHostnames: ["notes-1a2b3c.apps.example.com"],
          googleCallback: `https://${input.name}.apps.example.com/_konae/auth/google/callback` } as T;
      },
    };
    expect(await hostnameLocal(client, "d1")).toEqual(["This app answers at https://notes-1a2b3c.apps.example.com."]);
    await expect(hostnameLocal(client, "d1", "other")).rejects.toThrow(refusal);
    expect(await hostnameLocal(client, "d1", "notes")).toEqual([
      "This app now answers at https://notes.apps.example.com. notes-1a2b3c.apps.example.com redirects there with a 308 and stays this app's.",
      "App users sign in again on the new name. If the app offers Google sign-in, add https://notes.apps.example.com/_konae/auth/google/callback to its Google client's redirect URIs. Give the new address to any service that calls the app's HTTP routes.",
    ]);
  });

  test("a trailing --dir with no value is an error, not the current directory", async () => {
    const run = cli("build", "--dir");
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain("--dir needs a value");
  });

  test("init refuses a directory with files before it creates a project", async () => {
    await writeFile(path.join(directory, "app", "app.js"), "mine");
    const run = cli("init", "--dir", directory);
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain("Refusing to overwrite app");
    expect(calls.map((call) => call.name)).not.toContain("project_create");
  });

  test("a first release sends konae.json's signup policy and keeps it out of the project manifest", async () => {
    await writeFile(path.join(directory, "app", "app.js"), "console.log('hi');");
    const manifest = { visibility: "private", signupPolicy: { mode: "allowlist", allowlist: ["@example.test"] } };
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest } };
    const warnings: string[] = [];

    await publishLocal(directory, config, "token", (message) => warnings.push(message));

    const payload = JSON.parse(uploaded);
    expect(payload.initialSignupPolicy).toEqual({ mode: "allowlist", allowlist: ["@example.test"] });
    expect(payload.project.manifest).toEqual({ visibility: "private" });
    expect(calls.find((call) => call.name === "project_create")!.arguments.manifest).toEqual({ visibility: "private" });
    expect(calls.map((call) => call.name)).not.toContain("signup_policy_read");
    expect(warnings).toEqual([]);
  });

  test("a republish sends no policy and warns, without failing, when the app has another", async () => {
    await writeFile(path.join(directory, "app", "app.js"), "console.log('hi');");
    answers.project_inspect = { deployment: { id: "dep_1" } };
    answers.signup_policy_read = { deploymentId: "dep_1", mode: "closed", allowlist: [] };
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest: { signupPolicy: { mode: "invitation" } } } };
    const warnings: string[] = [];

    const receipt = await publishLocal(directory, config, "token", (message) => warnings.push(message));

    expect(receipt.url).toBe("http://test.localhost/");
    expect(JSON.parse(uploaded)).not.toHaveProperty("initialSignupPolicy");
    expect(calls.find((call) => call.name === "signup_policy_read")!.arguments).toEqual({ deploymentId: "dep_1" });
    expect(warnings).toEqual([
      "konae.json asks for signup Invitation only, but this app has Closed. A publish sets the policy only on the first release and never changes who may join afterwards. Change it in the editor's Access > Users view, or call signup_policy_set with a credential that holds manage_users.",
    ]);

    // The same policy, or a Kernel that cannot read it, publishes without a word.
    for (const answer of [{ mode: "invitation", allowlist: ["kept@example.test"] }, undefined]) {
      warnings.length = 0;
      if (answer) answers.signup_policy_read = answer; else failing.add("signup_policy_read");
      expect((await publishLocal(directory, config, "token", (message) => warnings.push(message))).url).toBe("http://test.localhost/");
      expect(warnings).toEqual([]);
    }
  });

  test("an allowlist differs when its entries do, ignoring case and order", () => {
    const wanted = { mode: "allowlist" as const, allowlist: ["B@example.test", "@team.test"] };
    expect(signupPolicyWarning(wanted, { mode: "allowlist", allowlist: ["@team.test", "b@example.test"] })).toBeUndefined();
    expect(signupPolicyWarning(wanted, { mode: "allowlist", allowlist: ["@team.test"] })).toContain("but this app has Allowlist (@team.test)");
  });

  test("a signup policy konae.json cannot hold is refused before anything is sent", async () => {
    expect(manifestSignupPolicy({})).toBeUndefined();
    expect(manifestSignupPolicy({ signupPolicy: { mode: "closed" } })).toEqual({ mode: "closed", allowlist: [] });
    expect(() => manifestSignupPolicy({ signupPolicy: { mode: "members" } })).toThrow('konae.json signupPolicy.mode must be "open", "invitation", "closed" or "allowlist"');
    expect(() => manifestSignupPolicy({ signupPolicy: { mode: "members" } })).not.toThrow("project.manifest");
    expect(() => manifestSignupPolicy({ signupPolicy: { mode: "allowlist", allowlist: "@example.test" } })).toThrow("must be a list of emails and @domains");
    await writeFile(path.join(directory, "app", "app.js"), "console.log('hi');");
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest: { signupPolicy: "invitation" } } };
    await expect(publishLocal(directory, config, "token")).rejects.toThrow("signupPolicy.mode");
    expect(calls).toEqual([]);
  });

  test("init writes a private app that admits the people its owner invites", async () => {
    await rm(path.join(directory, "app"), { recursive: true });
    const run = cli("init", "--dir", directory);
    expect(await run.exited).toBe(0);
    const written = JSON.parse(await readFile(path.join(directory, "konae.json"), "utf8"));
    expect(written).toMatchObject({ visibility: "private", signupPolicy: { mode: "invitation" }, signInMethods: ["password"] });
  });

  test("init writes the app folder and needs no Kernel and no credential", async () => {
    await rm(path.join(directory, "app"), { recursive: true });
    const run = Bun.spawn([process.execPath, "run", path.join(import.meta.dir, "konae.ts"), "init", "--dir", directory, "--name", "Field Notes!"], {
      env: Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))) as Record<string, string>, stdout: "pipe", stderr: "pipe",
    });
    expect(await run.exited).toBe(0);
    expect(calls).toEqual([]);
    const written = JSON.parse(await readFile(path.join(directory, "konae.json"), "utf8"));
    expect(written.url).toBeUndefined();
    expect(written).toMatchObject({ name: "Field Notes!", slug: "field-notes", kernel: rootPackage.version });
    expect(written.id).toMatch(/^app_[0-9a-f-]{36}$/);
    for (const file of ["index.html", "styles.css", "app.js", "schema.sql", "actions.json"]) expect((await readFile(path.join(directory, "app", file), "utf8")).length).toBeGreaterThan(0);
    for (const file of ["AGENTS.md", "action-contract.md", "actions.d.ts", "compose.yaml", ".env.example", "package.json", "scripts/build.mjs", "scripts/static-artifact.mjs", "scripts/runtime.js"]) {
      expect((await readFile(path.join(directory, file), "utf8")).length).toBeGreaterThan(0);
    }
    expect(await readFile(path.join(directory, ".gitignore"), "utf8")).toContain("data/");
  });

  test("init takes the editor's templates by id and refuses another before it writes", async () => {
    expect([...INIT_TEMPLATES]).toEqual(templateChoices.map((choice) => choice.id));
    // The file init and project_create both read is what the editor's templates render to.
    expect(await readFile(path.join(import.meta.dir, "..", PROJECT_TEMPLATES_FILE), "utf8"), "Run bun run templates:render.").toBe(renderProjectTemplates());
    await rm(path.join(directory, "app"), { recursive: true });
    const run = cli("init", "--dir", directory, "--template", "blog");
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain("Unknown template blog. Choose one of: tasks, notes, members");
    await expect(readFile(path.join(directory, "konae.json"), "utf8")).rejects.toThrow();
    // The shared list is for anyone with the link, so it is born public.
    expect(await cli("init", "--dir", directory, "--template", "tasks").exited).toBe(0);
    expect(JSON.parse(await readFile(path.join(directory, "konae.json"), "utf8")).visibility).toBe("public");
  });

  test("a publish goes to --url or the installation login saved", async () => {
    await writeFile(path.join(directory, "app", "app.js"), "console.log('hi');");
    const settings = { id: "app_test", name: "Test", slug: "test", visibility: "private", signupPolicy: { mode: "invitation", allowlist: [] }, signInMethods: ["password"], kernel: "0.1.0", manifest: { version: 1 } };
    const config = { project: { id: "app_test", name: "Test", slug: "test", manifest: { version: 1, visibility: "private" } } };
    await writeFile(path.join(directory, "konae.json"), JSON.stringify(settings));
    await expect(publishLocal(directory, config, "token")).rejects.toThrow(NO_INSTALLATION);
    const refused = cli("publish", "--dir", directory);
    expect(await refused.exited).toBe(1);
    expect(await new Response(refused.stderr).text()).toContain(NO_INSTALLATION);
    expect(calls).toEqual([]);

    await mkdir(path.join(directory, ".konae"));
    await writeFile(path.join(directory, ".konae/credentials.json"), JSON.stringify({ url: origin(), token: "token" }));
    expect(await cli("publish", "--dir", directory).exited).toBe(0);
    expect(calls.map((call) => call.name)).toContain("release_apply");
    expect(JSON.parse(await readFile(path.join(directory, "konae.json"), "utf8"))).toEqual(settings);
  });

  test("sql and events read the local app through the kernel binary unless --prod, and say so when there is none", async () => {
    await writeFile(path.join(directory, "konae.json"), JSON.stringify({ id: "app_test", name: "Test", slug: "test", visibility: "private", signupPolicy: { mode: "invitation" }, signInMethods: ["password"], kernel: "0.1.0", manifest: {} }));
    for (const command of [["sql", "SELECT 1"], ["events"]]) {
      const run = Bun.spawn([process.execPath, "run", path.join(import.meta.dir, "konae.ts"), command[0], "--dir", directory, ...command.slice(1)], {
        env: { ...process.env, KONAE_MCP_TOKEN: "token", KONAE_KERNEL_BIN: path.join(directory, "no-kernel") }, stdout: "pipe", stderr: "pipe",
      });
      expect(await run.exited).toBe(1);
      expect(await new Response(run.stderr).text()).toContain(`No kernel binary at ${path.join(directory, "no-kernel")}`);
    }
    expect(calls).toEqual([]);
    // --prod is the only way to the published database.
    answers.project_inspect = { deployment: { id: "dep_1" } };
    await mkdir(path.join(directory, ".konae"));
    await writeFile(path.join(directory, ".konae/credentials.json"), JSON.stringify({ url: origin(), token: "token" }));
    expect(await cli("sql", "--dir", directory, "--prod", "SELECT 1").exited).toBe(0);
    expect(calls.find((call) => call.name === "database_execute")!.arguments).toEqual({ deploymentId: "dep_1", operation: "sql", input: { sql: "SELECT 1" } });
    const empty = cli("sql", "--dir", directory);
    expect(await empty.exited).toBe(1);
    expect(await new Response(empty.stderr).text()).toContain("Give the statement in quotes or with --file");
    // events --prod reads the Deployment's record with the four owner tools.
    answers.usage_summary = { usage: { days: [] }, metrics: [], actions: [] };
    answers.calls_read = { calls: [] };
    answers.notices_read = { notices: [] };
    answers.signals_read = { signals: [{ name: "order.placed", at: 5, properties: { total: 3 } }] };
    calls.length = 0;
    const events = cli("events", "--dir", directory, "--prod", "--json", "--days", "3");
    expect(await events.exited).toBe(0);
    expect(JSON.parse(await new Response(events.stdout).text())).toMatchObject({ kind: "signal", title: "Signal order.placed" });
    expect(calls.filter((call) => call.name !== "project_inspect")).toEqual(expect.arrayContaining([
      { name: "usage_summary", arguments: { deploymentId: "dep_1", ...usageRange(3) } },
      { name: "calls_read", arguments: { deploymentId: "dep_1" } },
      { name: "notices_read", arguments: { deploymentId: "dep_1" } },
      { name: "signals_read", arguments: { deploymentId: "dep_1" } },
    ]));
  });

  test("events lists a development mailbox line as mail, with its recipient and text, and only the Deployment's own", () => {
    const mailbox = [
      JSON.stringify({ to: "owner@app.test", purpose: "notify", scope: "deployment:dep_1", action: "messages.send", subject: "New message", text: "Ana wrote:\nHola\n" }),
      JSON.stringify({ to: "ana@app.test", purpose: "recovery", scope: "deployment:dep_1", link: "http://app.localhost/_konae/account#recovery=abc" }),
      JSON.stringify({ to: "other@app.test", purpose: "notify", scope: "deployment:dep_2", action: "x", subject: "s", text: "t" }),
      JSON.stringify({ to: "creator@app.test", purpose: "verification", scope: "creator", link: "http://localhost/#verification=abc" }),
      "not json",
    ].join("\n");
    const seen = new Map<number, number>();
    const events = mailEvents(mailbox, "dep_1", seen, 1_000);
    expect(events.map((event) => event.title)).toEqual(["Email from messages.send to owner@app.test", "Account email (recovery) to ana@app.test"]);
    expect(events[1].mail!.link).toContain("#recovery=abc");
    // A later read keeps the time a line was first seen, so --follow prints it once.
    expect(mailEvents(mailbox, "dep_1", seen, 9_000).map((event) => event.at)).toEqual([1_000, 1_000]);
    expect(formatEvent(events[0])).toBe("00:00:01  mail      Email from messages.send to owner@app.test  (New message)\n          Written to the development mailbox. Nothing was sent.\n          | Ana wrote:\n          | Hola");
  });

  test("the events page answers on this machine with the mail and the Kernel's own record, and names a Kernel that fails", async () => {
    const mailbox = path.join(directory, "mailbox.jsonl");
    await writeFile(mailbox, `${JSON.stringify({ to: "owner@app.test", purpose: "notify", scope: "deployment:dep_1", action: "messages.send", subject: "New message", text: "<b>Hola</b>" })}\n`);
    let failing = false;
    const read: ReadRecord = async () => {
      if (failing) throw Error("The Kernel stopped.");
      return { summary: { usage: { days: [] }, metrics: [], actions: [] }, calls: [], notices: [], signals: [{ name: "order.placed", at: 5, properties: { total: 3 } }] } as unknown as Sources;
    };
    const feed = serveEvents(read, "dep_1", mailbox, 0);
    try {
      expect(new URL(feed.url).hostname).toBe("localhost");
      const page = await fetch(feed.url);
      expect(page.headers.get("content-security-policy")).toContain("default-src 'none'");
      // The page writes values as text, so a mail's markup stays text.
      expect(await page.text()).not.toContain("innerHTML");
      const events = await (await fetch(`${feed.url}/events.json`)).json() as { kind: string; title: string; mail?: { text: string } }[];
      expect(events.map((event) => event.kind)).toEqual(["signal", "mail"]);
      expect(events[1]).toMatchObject({ title: "Email from messages.send to owner@app.test", mail: { text: "<b>Hola</b>" } });
      failing = true;
      const failed = await fetch(`${feed.url}/events.json`);
      expect(failed.status).toBe(502);
      expect(await failed.json()).toEqual({ error: "The Kernel stopped." });
      expect((await fetch(`${feed.url}/other`)).status).toBe(404);
    } finally { feed.stop(); }
  });

  test("kernelEvents lists the same events from a record as from the four owner tools", async () => {
    const notice = (noticeId: string, state: "delivered" | "failed") => ({ noticeId, action: "orders.create", state, attempts: 1, errorKind: state === "failed" ? "rejected" : null, createdAt: 10, updatedAt: 11, nextAttemptAt: null });
    const record = {
      summary: { usage: { days: [], scheduled: [{ action: "digest.send", at: 20, outcome: "ok", changes: 2 }] }, metrics: [], actions: [] },
      calls: [{ callId: "c1", action: "orders.ship", state: "failed", attempts: 3, method: "POST", url: "https://api.example.test/ship", status: 500, error: "Server error", answer: null, createdAt: 30, updatedAt: 31, nextAttemptAt: null, then: null }],
      notices: [notice("n1", "delivered"), notice("n2", "failed")],
      signals: [{ name: "order.placed", at: 40, properties: { total: 3 } }],
    } as unknown as Sources;
    const asked: Array<{ name: string; input: Record<string, unknown> }> = [];
    const client = { async call<T>(name: string, input: Record<string, unknown>): Promise<T> {
      asked.push({ name, input });
      return ({ usage_summary: record.summary, calls_read: { calls: record.calls }, notices_read: { notices: record.notices }, signals_read: { signals: record.signals } } as Record<string, unknown>)[name] as T;
    } };
    const ranges: Array<{ from: string; to: string }> = [];
    const fromRecord: ReadRecord = async (range) => { ranges.push(range); return record; };

    for (const mailboxShowsDelivered of [false, true]) {
      const events = await kernelEvents(fromRecord, 3, mailboxShowsDelivered);
      expect(await kernelEvents(deploymentRecord(client, "dep_1"), 3, mailboxShowsDelivered)).toEqual(events);
      expect(events.map((event) => event.id)).toEqual(["signal:40:order.placed:0", "call:c1", "scheduled:20:digest.send:0", ...(mailboxShowsDelivered ? [] : ["notice:n1"]), "notice:n2"]);
    }
    expect(ranges).toEqual([usageRange(3), usageRange(3)]);
    expect(asked.slice(0, 4)).toEqual([
      { name: "usage_summary", input: { deploymentId: "dep_1", ...usageRange(3) } },
      { name: "calls_read", input: { deploymentId: "dep_1" } },
      { name: "notices_read", input: { deploymentId: "dep_1" } },
      { name: "signals_read", input: { deploymentId: "dep_1" } },
    ]);
    // Locally one run of the kernel binary's events command answers the range.
    const runs: string[][] = [];
    expect(await kernelRecord({ async run(args) { runs.push(args); return JSON.stringify(record); } })(usageRange(3))).toEqual(record);
    expect(runs).toEqual([["events", "--from", usageRange(3).from, "--to", usageRange(3).to]]);
  });

  test("tool calls one owner tool with its JSON arguments and prints the answer", async () => {
    answers.project_inspect = { deployment: { id: "dep_1" } };
    const run = cli("tool", "project_inspect", '{"projectId":"app_test"}', "--dir", directory, "--url", origin());
    expect(await run.exited).toBe(0);
    expect(await new Response(run.stdout).text()).toBe(`${JSON.stringify({ deployment: { id: "dep_1" } }, null, 2)}\n`);
    expect(calls).toEqual([{ name: "project_inspect", arguments: { projectId: "app_test" } }]);

    expect(await cli("tool", "--url", origin(), "session_inspect").exited).toBe(0);
    expect(calls.at(-1)).toEqual({ name: "session_inspect", arguments: {} });

    failing.add("secret_set");
    const refused = cli("tool", "secret_set", '{"projectId":"app_test","name":"hook","value":"v"}', "--url", origin());
    expect(await refused.exited).toBe(1);
    expect(await new Response(refused.stderr).text()).toBe("secret_set failed\n");

    const unnamed = cli("tool", "--url", origin());
    expect(await unnamed.exited).toBe(1);
    expect(await new Response(unnamed.stderr).text()).toContain("Name the tool");
    expect(HELP).toContain("set <name> --dir <folder> [--use route|call|google]");
  });

  test("tool --stdin reads one argument from standard input, so a secret stays off the command line", async () => {
    const piped = Bun.spawn([process.execPath, "run", path.join(import.meta.dir, "konae.ts"), "tool", "secret_set", '{"projectId":"app_test","name":"hook"}', "--stdin", "value", "--url", origin()], {
      env: { ...process.env, KONAE_MCP_TOKEN: "token" }, stdin: new Blob(["whsec_piped\n"]), stdout: "pipe", stderr: "pipe",
    });
    expect(await piped.exited).toBe(0);
    expect(calls).toEqual([{ name: "secret_set", arguments: { projectId: "app_test", name: "hook", value: "whsec_piped" } }]);
  });

  test("tool and domain talk to --url or the installation login saved, as publish does", async () => {
    // Nothing names the installation: the same refusal publish gives, not a default port.
    for (const run of [cli("tool", "session_inspect", "--dir", directory), cli("domain", "--deployment", "dep_1", "--dir", directory)]) {
      expect(await run.exited).toBe(1);
      expect(await new Response(run.stderr).text()).toBe(`${NO_INSTALLATION}\n`);
    }
    expect(calls).toEqual([]);
    // login saved the installation: tool reads it from the folder.
    await mkdir(path.join(directory, ".konae"));
    await writeFile(path.join(directory, ".konae/credentials.json"), JSON.stringify({ url: origin(), token: "saved" }));
    expect(await cli("tool", "session_inspect", "--dir", directory).exited).toBe(0);
    expect(calls).toEqual([{ name: "session_inspect", arguments: {} }]);
  });

  test("tool takes one JSON object as its arguments", () => {
    expect(toolArguments()).toEqual({});
    expect(toolArguments('{"deploymentId":"dep_1"}')).toEqual({ deploymentId: "dep_1" });
    expect(toolArguments('{"projectId":"app_1","name":"hook"}', { key: "value", value: "whsec_1\r\n" })).toEqual({ projectId: "app_1", name: "hook", value: "whsec_1" });
    expect(toolArguments(undefined, { key: "value", value: "v" })).toEqual({ value: "v" });
    for (const text of ["[1]", "null", "3", "{deploymentId}"]) expect(() => toolArguments(text)).toThrow(`The arguments must be one JSON object, such as '{"projectId":"app_1"}', not ${text}.`);
  });

  test("ignoreInGit adds an entry once", async () => {
    const file = path.join(directory, ".gitignore");
    await writeFile(file, "node_modules\n.konae/");
    await ignoreInGit(directory, [".konae/", "node_modules/", "dist/"]);
    await ignoreInGit(directory, [".konae/", "dist/"]);
    expect(await readFile(file, "utf8")).toBe("node_modules\n.konae/\ndist/\n");
  });
});
