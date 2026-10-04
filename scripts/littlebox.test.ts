import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { HELP, domainLocal, ignoreInGit, loginScopes, manifestSignupPolicy, publishLocal, signupPolicyWarning } from "./littlebox";

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
  directory = await mkdtemp(path.join(tmpdir(), "littlebox-cli-"));
});
afterEach(async () => { await rm(directory, { recursive: true, force: true }); });

const origin = () => `http://127.0.0.1:${server.port}`;

function cli(...args: string[]) {
  return Bun.spawn([process.execPath, "run", path.join(import.meta.dir, "littlebox.ts"), ...args], {
    env: { ...process.env, LITTLEBOX_MCP_TOKEN: "token" },
    stdout: "pipe",
    stderr: "pipe",
  });
}

describe("littlebox CLI", () => {
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
    const html = JSON.parse(await readFile(path.join(directory, ".littlebox/artifact.json"), "utf8")).files["index.html"] as string;
    expect(html).toContain("<script data-littlebox-runtime>");
  });

  test("login adds manage_users, export_data and manage_lifecycle only when a flag asks", () => {
    expect(loginScopes([])).toEqual(["read", "write", "publish"]);
    expect(loginScopes(["--manage-users"])).toEqual(["read", "write", "publish", "manage_users"]);
    expect(loginScopes(["--manage-lifecycle", "--export-data", "--manage-users"])).toEqual(["read", "write", "publish", "manage_users", "export_data", "manage_lifecycle"]);
  });

  test("help names every command and every login flag, and an unknown command prints it", async () => {
    for (const word of ["login", "init", "dev", "build", "contract", "publish", "conform", "export", "eject", "--manage-users", "--export-data", "--manage-lifecycle", "--with-data", "--project", "--revision", "--secret", "--key", "--inputs", "domain", "--deployment", "--set", "--clear"]) expect(HELP).toContain(word);
    const run = cli("deploy");
    expect(await run.exited).toBe(1);
    expect(await new Response(run.stderr).text()).toContain("Unknown command deploy.\n\nUsage: bun run littlebox --");
  });

  test("domain checks the hostname by the Kernel's rules before domain_set and prints the TXT record", async () => {
    const calls: string[] = [];
    const rules = { suffix: "apps.example.com", installationHosts: ["littlebox.example.com"] };
    let state: Record<string, unknown> = { hostname: null, status: "none", generatedUrl: "https://notes-1a2b3c.apps.example.com", rules };
    const client = {
      async call<T>(name: string, input: Record<string, unknown>): Promise<T> {
        calls.push(`${name} ${input.hostname ?? ""}`.trim());
        if (name === "domain_set") {
          state = { ...state, hostname: input.hostname, status: calls.filter((call) => call.startsWith("domain_set")).length > 1 ? "active" : "pending",
            url: `https://${input.hostname}`, dns: "No TXT record at _littlebox.notes.example.org yet.",
            record: { type: "TXT", name: `_littlebox.${input.hostname}`, value: "littlebox-verify-abc" } };
        }
        if (name === "domain_clear") return { cleared: state.hostname, generatedUrl: state.generatedUrl } as T;
        return state as T;
      },
    };
    await expect(domainLocal(client, "d1", { set: "shop.apps.example.com" })).rejects.toThrow(
      "shop.apps.example.com is under this installation's app suffix apps.example.com, where Littlebox names apps itself. Name a hostname outside it.");
    expect(calls).toEqual(["domain_read"]);
    expect(await domainLocal(client, "d1", { set: "Notes.Example.org" })).toEqual([
      "notes.example.org does not serve yet. No TXT record at _littlebox.notes.example.org yet.",
      'Add this record at your DNS provider: TXT _littlebox.notes.example.org "littlebox-verify-abc"',
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

  test("a first release sends littlebox.json's signup policy and keeps it out of the project manifest", async () => {
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
      "littlebox.json asks for signup Invitation only, but this app has Closed. A publish sets the policy only on the first release and never changes who may join afterwards. Change it in the editor's Auth > Users view, or call signup_policy_set with a credential that holds manage_users.",
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

  test("a signup policy littlebox.json cannot hold is refused before anything is sent", async () => {
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
    const written = JSON.parse(await readFile(path.join(directory, "littlebox.json"), "utf8"));
    expect(written.project.manifest).toMatchObject({ visibility: "private", signupPolicy: { mode: "invitation" } });
  });

  test("ignoreInGit adds an entry once", async () => {
    const file = path.join(directory, ".gitignore");
    await writeFile(file, "node_modules\n.littlebox/");
    await ignoreInGit(directory, [".littlebox/", "node_modules/", "dist/"]);
    await ignoreInGit(directory, [".littlebox/", "dist/"]);
    expect(await readFile(file, "utf8")).toBe("node_modules\n.littlebox/\ndist/\n");
  });
});
