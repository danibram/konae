import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";
import { INIT_TEMPLATES, initLocal, publishLocal, setDevSecrets } from "../scripts/littlebox";
import { installPulledAlone, pullLocal, readPulled } from "../scripts/pull";

// init, publish, pull and the first release of a Development Kernel, on two
// real Kernels: `origin` stands for the installation an app is published to
// and `local` for the one `littlebox dev` starts.
type Kernel = { url: string; process: ReturnType<typeof Bun.spawn>; owner: McpClient; token: string };
let root: string;
let origin: Kernel;
let local: Kernel;
const password = "pull-integration-password";
let nextClient = 0;
const freshClient = () => { nextClient++; return `198.19.${Math.floor(nextClient / 250)}.${nextClient % 250 + 1}`; };

async function start(name: string, port: number): Promise<Kernel> {
  const url = `http://127.0.0.1:${port}`;
  const setupKey = `pull-integration-${name}-setup-secret`;
  const process_ = Bun.spawn(["kernel-rs/target/debug/littlebox-kernel"], {
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_"))), LITTLEBOX_DATA_DIR: path.join(root, name), LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_ADMIN_KEY: setupKey, LITTLEBOX_PUBLIC_ORIGIN: url, LITTLEBOX_HOST_SUFFIX: "localhost", LITTLEBOX_BUN_PATH: process.execPath, LITTLEBOX_TRUSTED_PROXIES: "127.0.0.1" },
    stdout: "ignore", stderr: "inherit",
  });
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_littlebox/health`)).ok) break; } catch {}
    if (process_.exitCode !== null) throw Error(`The ${name} Kernel stopped.`);
    await Bun.sleep(100);
  }
  const response = await fetch(`${url}/_littlebox/owner/signup`, { method: "POST", headers: { "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email: `owner@${name}.test`, password, setupKey }) });
  expect(response.status).toBe(200);
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  const owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  const { token } = await owner.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish", "manage_users", "export_data"], ttlSeconds: 600 });
  return { url, process: process_, owner, token };
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "littlebox-pull-"));
  const port = Number(process.env.LITTLEBOX_PULL_TEST_PORT) || 19900 + Math.floor(Math.random() * 80);
  origin = await start("origin", port);
  local = await start("local", port + 1);
}, 300_000);

afterAll(async () => {
  for (const kernel of [origin, local]) { kernel?.process.kill(); if (kernel) await kernel.process.exited; }
  if (root) await rm(root, { recursive: true, force: true });
});

/** The app's runtime endpoint for one signed-in account, which `route` creates or signs in. */
async function member(kernel: Kernel, hostname: string, route: "signup" | "login") {
  const response = await fetch(`${kernel.url}/_littlebox/auth/${route}`, { method: "POST", headers: { host: hostname, "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email: "ana@pull.test", password }) });
  expect(response.status).toBe(200);
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  return new McpClient(`${kernel.url}/_littlebox/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", hostname); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

const hostOf = (receipt: { url: string }) => new URL(receipt.url).hostname;

describe("every template init offers", () => {
  for (const template of INIT_TEMPLATES) {
    test(`${template} publishes on a Kernel with no data and serves its page`, async () => {
      const folder = path.join(root, `template-${template}`);
      const config = await initLocal(folder, { name: `Template ${template}`, template });
      const admin = new McpClient(`${local.url}/mcp`, { token: local.token });
      const { signupPolicy: _, ...manifest } = config.project.manifest;
      const set = await setDevSecrets(folder, admin, { ...config.project, manifest });
      // Only the webhook receiver signs, and a second run finds its secret held.
      expect(set).toHaveLength(template === "webhook" ? 1 : 0);
      expect(await setDevSecrets(folder, admin, { ...config.project, manifest })).toEqual([]);
      const receipt = await publishLocal(folder, { ...config, url: local.url }, local.token, () => {});
      const page = await fetch(`${local.url}/`, { headers: { host: hostOf(receipt), "x-forwarded-for": freshClient() }, redirect: "manual" });
      // A public app answers with its page and a private one with the Kernel's sign-in page.
      expect(page.status).toBe(200);
    }, 60_000);
  }
});

describe("a project starts the same through MCP and through init", () => {
  test("project_create answers the files init writes, the default and a named template, with the template's visibility", async () => {
    const read = async (folder: string) => Object.fromEntries(await Promise.all(["actions.json", "app.js", "index.html", "schema.sql", "styles.css"].map(async (name) => [name, await readFile(path.join(folder, name), "utf8")])));
    const notes = path.join(root, "mcp-notes");
    await initLocal(notes);
    const created = await origin.owner.call<{ files: Record<string, string>; manifest: { visibility: string }; sourceAuthority: string }>("project_create", { name: "From MCP" });
    expect(created.files).toEqual(await read(notes));
    expect(created).toMatchObject({ manifest: { visibility: "private" }, sourceAuthority: "external" });

    const tasks = path.join(root, "mcp-tasks");
    const config = await initLocal(tasks, { template: "tasks" });
    const shared = await origin.owner.call<{ files: Record<string, string>; manifest: { visibility: string } }>("project_create", { name: "Shared from MCP", template: "tasks" });
    expect(shared.files).toEqual(await read(tasks));
    expect(shared.manifest.visibility).toBe(config.project.manifest.visibility);
    expect(shared.manifest.visibility).toBe("public");
    await expect(origin.owner.call("project_create", { name: "Unknown", template: "blog" })).rejects.toThrow();
  }, 60_000);
});

describe("init, publish, pull, dev", () => {
  let app: string;
  let config: Awaited<ReturnType<typeof initLocal>>;

  test("a project init wrote without a Kernel publishes, and its rows and accounts arrive on a Kernel with no data", async () => {
    app = path.join(root, "app");
    config = await initLocal(app, { name: "Pulled notes" });
    expect(config.url).toBeUndefined();

    const published = await publishLocal(app, { ...config, url: origin.url }, origin.token);
    expect(published.first).toBe(true);
    await origin.owner.call("signup_policy_set", { deploymentId: published.id, mode: "open" });
    const ana = await member(origin, hostOf(published), "signup");
    await ana.call("actions_invoke", { name: "notes.create", input: { title: "Written on the origin" } });

    const pulled = await pullLocal(app, new McpClient(`${origin.url}/mcp`, { token: origin.token }), origin.url, config.project.id);
    expect(pulled.manifest.contents.map((content) => content.name)).toEqual(expect.arrayContaining(["database", "identity"]));
    expect(await readPulled(app)).toEqual(pulled);

    const first = await publishLocal(app, { ...config, url: local.url }, local.token, () => {}, pulled);
    expect(first.first).toBe(true);
    // The account signs in with the password it had on the origin and reads its own row.
    const there = await member(local, hostOf(first), "login");
    expect(await there.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Written on the origin" }]);

    // A later release of the same project carries no package, and the row stays.
    const again = await publishLocal(app, { ...config, url: local.url }, local.token, () => {}, pulled);
    expect(again.first).toBe(false);
    expect(await there.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Written on the origin" }]);
    // The origin is untouched by any of it.
    expect(await ana.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Written on the origin" }]);
  }, 120_000);

  test("a pull without export_data names the login flag, and a project without a Deployment says to publish", async () => {
    const { token } = await origin.owner.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish"], ttlSeconds: 600 });
    await expect(pullLocal(app, new McpClient(`${origin.url}/mcp`, { token }), origin.url, config.project.id)).rejects.toThrow("login with --export-data");
    await origin.owner.call("project_create", { projectId: "pull_unpublished", name: "Unpublished" });
    await expect(pullLocal(app, new McpClient(`${origin.url}/mcp`, { token: origin.token }), origin.url, "pull_unpublished")).rejects.toThrow("Publish it first");
    // The refused pulls left the earlier copy where it was.
    expect((await readPulled(app))?.manifest.projectId).toBe(config.project.id);
  }, 60_000);

  test("actions that need a newer schema than the copy are refused, the copy installs alone, and the same SQL then lets the app publish", async () => {
    const next = path.join(root, "next");
    const born = await initLocal(next, { name: "Next version" });
    const published = await publishLocal(next, { ...born, url: origin.url }, origin.token);
    await origin.owner.call("signup_policy_set", { deploymentId: published.id, mode: "open" });
    await (await member(origin, hostOf(published), "signup")).call("actions_invoke", { name: "notes.create", input: { title: "Before the column" } });
    const pulled = await pullLocal(next, new McpClient(`${origin.url}/mcp`, { token: origin.token }), origin.url, born.project.id);

    const actions = await readFile(path.join(next, "actions.json"), "utf8");
    expect(actions).toContain("SELECT id, title, created_at FROM notes");
    await writeFile(path.join(next, "actions.json"), actions.replace("SELECT id, title, created_at FROM notes", "SELECT id, title, pinned, created_at FROM notes"));

    const admin = new McpClient(`${local.url}/mcp`, { token: local.token });
    await expect(publishLocal(next, { ...born, url: local.url }, local.token, () => {}, pulled)).rejects.toThrow("no such column: pinned");
    expect((await admin.call<{ deployment: unknown }>("project_inspect", { projectId: born.project.id })).deployment).toBeNull();

    const { signupPolicy: _, ...manifest } = born.project.manifest;
    const alone = await installPulledAlone(admin, next, pulled, { ...born.project, manifest });
    await admin.call("database_execute", { deploymentId: alone.id, operation: "sql", input: { sql: "ALTER TABLE notes ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0" } });
    const released = await publishLocal(next, { ...born, url: local.url }, local.token, () => {});
    expect(released.id).toBe(alone.id);
    const there = await member(local, hostOf(released), "login");
    expect(await there.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Before the column", pinned: 0 }]);
  }, 120_000);
});
