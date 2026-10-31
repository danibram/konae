import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { templateChoices } from "../src/lib/templates";
import { PROJECT_TEMPLATES_FILE, renderProjectTemplates } from "./project-templates";
import { formatEvent, mailEvents, serveEvents } from "./events";
import { HELP, INIT_TEMPLATES, NO_INSTALLATION, VERIFY_BY_LINK, creatorVerificationToken, domainLocal, ignoreInGit, isLoopbackUrl, loginScopes, manifestSignupPolicy, publishLocal, signupPolicyWarning } from "./konae";

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
        release_plan: { planId: "plan_1" },
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

describe("konae CLI", () => {
  test.each(["release_plan", "release_apply"])("a failed %s releases its upload so later publishes can proceed", async (step) => {
    await writeFile(path.join(directory, "app.js"), "console.log('hi');");
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
    await writeFile(path.join(directory, "app.js"), "console.log('hi');");
    failing.add("release_plan");
    failing.add("transfer_cancel");
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest: {} } };

    await expect(publishLocal(directory, config, "token")).rejects.toThrow("release_plan failed");
  });

  test("a publish whose upload cannot be released afterwards still reports the release", async () => {
    await writeFile(path.join(directory, "app.js"), "console.log('hi');");
    await writeFile(path.join(directory, "index.html"), "<main id=app></main>");
    failing.add("transfer_cancel");
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest: {} } };

    const receipt = await publishLocal(directory, config, "token");

    expect(receipt.url).toBe("http://test.localhost/");
    expect(calls.map((call) => call.name)).toContain("transfer_cancel");
    // No visibility in the manifest publishes private, the same default as the exported deploy script.
    expect(JSON.parse(uploaded).visibility).toBe("private");
    const html = JSON.parse(await readFile(path.join(directory, ".konae/artifact.json"), "utf8")).files["index.html"] as string;
    expect(html).toContain("<script data-konae-runtime>");
  });

  test("login adds manage_users and export_data only when a flag asks", () => {
    expect(loginScopes([])).toEqual(["read", "write", "publish"]);
    expect(loginScopes(["--manage-users"])).toEqual(["read", "write", "publish", "manage_users"]);
    expect(loginScopes(["--export-data", "--manage-users"])).toEqual(["read", "write", "publish", "manage_users", "export_data"]);
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

  test("help names every command and every login flag, and an unknown command prints it", async () => {
    for (const word of ["login", "init", "dev", "build", "contract", "publish", "conform", "export", "--manage-users", "--export-data", "--verify-from", "--with-data", "--secret", "--key", "--inputs", "domain", "--deployment", "--set", "--clear", "pull", "sql", "events", "--prod", "--follow", "--json", "--events-port", "KONAE_KERNEL_BIN", "--file", "--reset", "--template"]) expect(HELP).toContain(word);
    const run = cli("deploy");
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain("Unknown command deploy.\n\nUsage: bun run konae --");
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
      "shop.apps.example.com is under this installation's app suffix apps.example.com, where Konae names apps itself. Name a hostname outside it.");
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

  test("a trailing --dir with no value is an error, not the current directory", async () => {
    const run = cli("build", "--dir");
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain("--dir needs a value");
  });

  test("init refuses a directory with files before it creates a project", async () => {
    await writeFile(path.join(directory, "app.js"), "mine");
    const run = cli("init", "--dir", directory, "--url", origin());
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain("Refusing to overwrite app.js");
    expect(calls.map((call) => call.name)).not.toContain("project_create");
  });

  test("a first release sends konae.json's signup policy and keeps it out of the project manifest", async () => {
    await writeFile(path.join(directory, "app.js"), "console.log('hi');");
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
    await writeFile(path.join(directory, "app.js"), "console.log('hi');");
    answers.project_inspect = { deployment: { id: "dep_1" } };
    answers.signup_policy_read = { deploymentId: "dep_1", mode: "closed", allowlist: [] };
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest: { signupPolicy: { mode: "invitation" } } } };
    const warnings: string[] = [];

    const receipt = await publishLocal(directory, config, "token", (message) => warnings.push(message));

    expect(receipt.url).toBe("http://test.localhost/");
    expect(JSON.parse(uploaded)).not.toHaveProperty("initialSignupPolicy");
    expect(calls.find((call) => call.name === "signup_policy_read")!.arguments).toEqual({ deploymentId: "dep_1" });
    expect(warnings).toEqual([
      "konae.json asks for signup Invitation only, but this app has Closed. A publish sets the policy only on the first release and never changes who may join afterwards. Change it in the editor's Auth > Users view, or call signup_policy_set with a credential that holds manage_users.",
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
    expect(() => manifestSignupPolicy({ signupPolicy: { mode: "members" } })).toThrow('signupPolicy.mode must be "open", "invitation", "closed" or "allowlist"');
    expect(() => manifestSignupPolicy({ signupPolicy: { mode: "allowlist", allowlist: "@example.test" } })).toThrow("must be a list of emails and @domains");
    await writeFile(path.join(directory, "app.js"), "console.log('hi');");
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest: { signupPolicy: "invitation" } } };
    await expect(publishLocal(directory, config, "token")).rejects.toThrow("signupPolicy.mode");
    expect(calls).toEqual([]);
  });

  test("init writes a private app that admits the people its owner invites", async () => {
    const run = cli("init", "--dir", directory, "--url", origin());
    expect(await run.exited).toBe(0);
    const written = JSON.parse(await readFile(path.join(directory, "konae.json"), "utf8"));
    expect(written.project.manifest).toMatchObject({ visibility: "private", signupPolicy: { mode: "invitation" } });
  });

  test("init needs no Kernel and no credential, and names no installation unless --url does", async () => {
    const run = Bun.spawn([process.execPath, "run", path.join(import.meta.dir, "konae.ts"), "init", "--dir", directory, "--name", "Field Notes!"], {
      env: Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))) as Record<string, string>, stdout: "pipe", stderr: "pipe",
    });
    expect(await run.exited).toBe(0);
    expect(calls).toEqual([]);
    const written = JSON.parse(await readFile(path.join(directory, "konae.json"), "utf8"));
    expect(written.url).toBeUndefined();
    expect(written.project).toMatchObject({ name: "Field Notes!", slug: "field-notes" });
    expect(written.project.id).toMatch(/^app_[0-9a-f-]{36}$/);
    for (const file of ["index.html", "styles.css", "app.js", "schema.sql", "actions.json"]) expect((await readFile(path.join(directory, file), "utf8")).length).toBeGreaterThan(0);
  });

  test("init takes the editor's templates by id and refuses another before it writes", async () => {
    expect([...INIT_TEMPLATES]).toEqual(templateChoices.map((choice) => choice.id));
    // The file init and project_create both read is what the editor's templates render to.
    expect(await readFile(path.join(import.meta.dir, "..", PROJECT_TEMPLATES_FILE), "utf8"), "Run bun run templates:render.").toBe(renderProjectTemplates());
    const run = cli("init", "--dir", directory, "--template", "blog");
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain("Unknown template blog. Choose one of: tasks, notes, members");
    await expect(readFile(path.join(directory, "konae.json"), "utf8")).rejects.toThrow();
    // The shared list is for anyone with the link, so it is born public.
    expect(await cli("init", "--dir", directory, "--template", "tasks").exited).toBe(0);
    expect(JSON.parse(await readFile(path.join(directory, "konae.json"), "utf8")).project.manifest.visibility).toBe("public");
  });

  test("a publish with no installation in konae.json goes to the one login saved, and keeps it", async () => {
    await writeFile(path.join(directory, "app.js"), "console.log('hi');");
    const config = { project: { id: "app_test", name: "Test", slug: "test", manifest: { visibility: "private" } } };
    await writeFile(path.join(directory, "konae.json"), JSON.stringify(config));
    await expect(publishLocal(directory, config, "token")).rejects.toThrow(NO_INSTALLATION);
    const refused = cli("publish", "--dir", directory);
    expect(await refused.exited).toBe(1);
    expect(await new Response(refused.stderr).text()).toContain(NO_INSTALLATION);
    expect(calls).toEqual([]);

    await mkdir(path.join(directory, ".konae"));
    await writeFile(path.join(directory, ".konae/credentials.json"), JSON.stringify({ url: origin(), token: "token" }));
    expect(await cli("publish", "--dir", directory).exited).toBe(0);
    expect(calls.map((call) => call.name)).toContain("release_apply");
    expect(JSON.parse(await readFile(path.join(directory, "konae.json"), "utf8"))).toEqual({ url: origin(), ...config });
  });

  test("a pulled copy goes with a first release as its package, and never with a later one", async () => {
    await writeFile(path.join(directory, "app.js"), "console.log('hi');");
    const bytes = new TextEncoder().encode("database bytes");
    const sha256 = new Bun.CryptoHasher("sha256").update(bytes).digest("hex");
    const manifest = { format: "konae-package", version: 1, packageId: "pkg_1", createdAt: 1, deploymentId: "dep_origin", projectId: "app_test", visibility: "public", publishedRevision: 1, contents: [{ name: "database", size: bytes.length, sha256 }], excluded: [] };
    await mkdir(path.join(directory, ".konae/pull"), { recursive: true });
    await writeFile(path.join(directory, ".konae/pull/database.sqlite"), bytes);
    const pulled = { manifest, from: "http://origin.test", at: 1 } as Parameters<typeof publishLocal>[4];
    const config = { url: origin(), project: { id: "app_test", name: "Test", slug: "test", manifest: { visibility: "private" } } };

    const first = await publishLocal(directory, config, "token", () => {}, pulled);
    expect(first.first).toBe(true);
    expect(calls.find((call) => call.name === "transfer_begin")!.arguments).toMatchObject({ component: "database", size: bytes.length, sha256, projectId: "app_test" });
    const payload = JSON.parse(uploaded.slice(bytes.length));
    expect(payload.initialPackage).toEqual({ manifest, components: { database: "transfer_1" } });
    // The copy says who may open the app it was taken from.
    expect(payload.visibility).toBe("public");

    calls.length = 0; uploaded = "";
    answers.project_inspect = { deployment: { id: "dep_1" } };
    const later = await publishLocal(directory, config, "token", () => {}, pulled);
    expect(later.first).toBe(false);
    expect(calls.filter((call) => call.name === "transfer_begin")).toHaveLength(1);
    expect(JSON.parse(uploaded).initialPackage).toBeUndefined();
  });

  test("sql and events reach the Development Kernel unless --prod, and say so when none runs", async () => {
    await writeFile(path.join(directory, "konae.json"), JSON.stringify({ project: { id: "app_test", name: "Test", slug: "test", manifest: {} } }));
    for (const command of [["sql", "SELECT 1"], ["events"]]) {
      const run = cli(command[0], "--dir", directory, ...command.slice(1));
      expect(await run.exited).toBe(1);
      const refusal = await new Response(run.stderr).text();
      expect(refusal).toContain("No Development Kernel is running for this folder.");
      expect(refusal).toContain("add --prod");
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
    const kernel = { failing: false, async call(name: string) {
      if (this.failing) throw Error("The Kernel stopped.");
      return ({ usage_summary: { usage: { days: [] }, metrics: [], actions: [] }, calls_read: { calls: [] }, notices_read: { notices: [] }, signals_read: { signals: [{ name: "order.placed", at: 5, properties: { total: 3 } }] } } as Record<string, unknown>)[name];
    } };
    const feed = serveEvents(kernel as never, "dep_1", mailbox, 0);
    try {
      expect(new URL(feed.url).hostname).toBe("localhost");
      const page = await fetch(feed.url);
      expect(page.headers.get("content-security-policy")).toContain("default-src 'none'");
      // The page writes values as text, so a mail's markup stays text.
      expect(await page.text()).not.toContain("innerHTML");
      const events = await (await fetch(`${feed.url}/events.json`)).json() as { kind: string; title: string; mail?: { text: string } }[];
      expect(events.map((event) => event.kind)).toEqual(["signal", "mail"]);
      expect(events[1]).toMatchObject({ title: "Email from messages.send to owner@app.test", mail: { text: "<b>Hola</b>" } });
      kernel.failing = true;
      const failed = await fetch(`${feed.url}/events.json`);
      expect(failed.status).toBe(502);
      expect(await failed.json()).toEqual({ error: "The Kernel stopped." });
      expect((await fetch(`${feed.url}/other`)).status).toBe(404);
    } finally { feed.stop(); }
  });

  test("ignoreInGit adds an entry once", async () => {
    const file = path.join(directory, ".gitignore");
    await writeFile(file, "node_modules\n.konae/");
    await ignoreInGit(directory, [".konae/", "node_modules/", "dist/"]);
    await ignoreInGit(directory, [".konae/", "dist/"]);
    expect(await readFile(file, "utf8")).toBe("node_modules\n.konae/\ndist/\n");
  });
});
