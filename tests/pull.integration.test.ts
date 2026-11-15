import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { cp, mkdir, mkdtemp, readFile, readdir, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient, type Transfer } from "../src/mcp/client";
import { readTar } from "../src/lib/tar";
import { INIT_TEMPLATES, buildLocal, configOf, initLocal, localKernel, publishLocal, readSettings, resetData, setDevSecrets } from "../scripts/konae";
import { pullLocal, readPulled } from "../scripts/pull";
import { provideLocalAppDependencies } from "./local-app-deps";

// init, publish, pull and dev --reset: `origin` is a Konae installation an
// app is published to, and each folder runs locally on the kernel binary, as
// `konae dev` runs it, with the data pull exported in data/.
type Kernel = { url: string; process: ReturnType<typeof Bun.spawn>; owner: McpClient; token: string };
let root: string;
let origin: Kernel;
let localPort: number;
let served: ReturnType<typeof Bun.spawn> | undefined;
const password = "pull-integration-password";
let nextClient = 0;
const freshClient = () => { nextClient++; return `198.19.${Math.floor(nextClient / 250)}.${nextClient % 250 + 1}`; };

async function start(name: string, port: number): Promise<Kernel> {
  const url = `http://127.0.0.1:${port}`;
  const setupKey = `pull-integration-${name}-setup-secret`;
  const process_ = Bun.spawn(["kernel-rs/target/debug/konae"], {
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))), KONAE_DATA_DIR: path.join(root, name), KONAE_KERNEL_PORT: String(port), KONAE_BIND: "127.0.0.1", KONAE_SETUP_KEY: setupKey, KONAE_PUBLIC_ORIGIN: url, KONAE_HOST_SUFFIX: "localhost", KONAE_TRUSTED_PROXIES: "127.0.0.1" },
    stdout: "ignore", stderr: "inherit",
  });
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_konae/health`)).ok) break; } catch {}
    if (process_.exitCode !== null) throw Error(`The ${name} Kernel stopped.`);
    await Bun.sleep(100);
  }
  const response = await fetch(`${url}/_konae/owner/signup`, { method: "POST", headers: { "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email: `owner@${name}.test`, password, setupKey }) });
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
  root = await mkdtemp(path.join(tmpdir(), "konae-pull-"));
  const port = Number(process.env.KONAE_PULL_TEST_PORT) || 19900 + Math.floor(Math.random() * 80);
  origin = await start("origin", port);
  localPort = port + 1;
}, 300_000);

afterAll(async () => {
  origin?.process.kill(); if (origin) await origin.process.exited;
  await stopLocal();
  if (root) await rm(root, { recursive: true, force: true });
});

async function stopLocal() {
  if (served && served.exitCode === null) { served.kill(); await served.exited; }
  served = undefined;
}

/** Builds the folder and serves it on the kernel binary, as dev does. Answers the local Kernel and the app's address. */
async function serveLocally(folder: string) {
  await stopLocal();
  const kernel = await localKernel(folder, await readSettings(folder), { port: String(localPort), build: false });
  await buildLocal(folder);
  const set = await setDevSecrets(folder, kernel);
  served = Bun.spawn([kernel.binary], { env: { ...kernel.env, KONAE_TRUSTED_PROXIES: "127.0.0.1" }, stdout: "pipe", stderr: "inherit" });
  let output = "";
  for await (const block of served.stdout as ReadableStream<Uint8Array>) { output += new TextDecoder().decode(block); if (output.includes("listening on")) break; }
  const url = /Serving .+ at (\S+)/.exec(output)?.[1];
  if (!url) throw Error(`The kernel binary did not serve ${folder}:\n${output}`);
  return { url, set, kernel, local: { url: `http://127.0.0.1:${localPort}` } as Kernel };
}

/** Builds the folder and imports the pulled data with the kernel binary's import, as dev --reset does. */
async function importPulled(folder: string, pulled: Awaited<ReturnType<typeof pullLocal>>) {
  await stopLocal();
  const kernel = await localKernel(folder, await readSettings(folder), { port: String(localPort), build: false });
  await buildLocal(folder);
  await resetData(folder, kernel, pulled);
}

/** The app's runtime endpoint for one signed-in account, which `route` creates or signs in. */
async function member(kernel: Kernel, hostname: string, route: "signup" | "login") {
  const response = await fetch(`${kernel.url}/_konae/auth/${route}`, { method: "POST", headers: { host: hostname, "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email: "ana@pull.test", password }) });
  expect(response.status).toBe(200);
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  return new McpClient(`${kernel.url}/_konae/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", hostname); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

const hostOf = (receipt: { url: string }) => new URL(receipt.url).hostname;

describe("every template init offers", () => {
  for (const template of INIT_TEMPLATES) {
    test(`${template} runs on the kernel binary with no data and serves its page`, async () => {
      const folder = path.join(root, `template-${template}`);
      await initLocal(folder, { name: `Template ${template}`, template });
      await provideLocalAppDependencies(folder);
      const { url, set, kernel, local } = await serveLocally(folder);
      // Only the webhook receiver signs, and a second run finds its secret held.
      expect(set).toHaveLength(template === "webhook" ? 1 : 0);
      expect(await setDevSecrets(folder, kernel)).toEqual([]);
      const page = await fetch(`${local.url}/`, { headers: { host: hostOf({ url }), "x-forwarded-for": freshClient() }, redirect: "manual" });
      // A public app answers with its page and a private one with the Kernel's sign-in page.
      expect(page.status).toBe(200);
    }, 60_000);
  }
});

describe("a project starts the same through MCP and through init", () => {
  test("project_create answers the files init writes, the default and a named template, with the template's visibility", async () => {
    const read = async (folder: string) => Object.fromEntries(await Promise.all(["actions.json", "app.js", "index.html", "schema.sql", "styles.css"].map(async (name) => [name, await readFile(path.join(folder, "app", name), "utf8")])));
    const notes = path.join(root, "mcp-notes");
    configOf(await initLocal(notes));
    await provideLocalAppDependencies(notes);
    const created = await origin.owner.call<{ files: Record<string, string>; manifest: { visibility: string } }>("project_create", { name: "From MCP" });
    expect(created.files).toEqual(await read(notes));
    expect(created).toMatchObject({ manifest: { visibility: "private" } });

    const tasks = path.join(root, "mcp-tasks");
    const config = configOf(await initLocal(tasks, { template: "tasks" }));
    await provideLocalAppDependencies(tasks);
    const shared = await origin.owner.call<{ files: Record<string, string>; manifest: { visibility: string } }>("project_create", { name: "Shared from MCP", template: "tasks" });
    expect(shared.files).toEqual(await read(tasks));
    expect(shared.manifest.visibility).toBe(config.project.manifest.visibility);
    expect(shared.manifest.visibility).toBe("public");
    await expect(origin.owner.call("project_create", { name: "Unknown", template: "blog" })).rejects.toThrow();
  }, 60_000);
});

describe("init, publish, pull, dev --reset", () => {
  let app: string;
  let config: Awaited<ReturnType<typeof initLocal>>;

  test("a project init wrote without a Kernel publishes, and its rows and accounts arrive in data/", async () => {
    app = path.join(root, "app");
    config = configOf(await initLocal(app, { name: "Pulled notes" }));
    await provideLocalAppDependencies(app);
    expect(config.url).toBeUndefined();

    const published = await publishLocal(app, { ...config, url: origin.url }, origin.token);
    expect(published.first).toBe(true);
    await origin.owner.call("signup_policy_set", { deploymentId: published.id, mode: "open" });
    const ana = await member(origin, hostOf(published), "signup");
    await ana.call("actions_invoke", { name: "notes.create", input: { title: "Written on the origin" } });

    const pulled = await pullLocal(app, new McpClient(`${origin.url}/mcp`, { token: origin.token }), origin.url, config.project.id);
    expect(pulled.manifest.contents.map((content) => content.name)).toEqual(expect.arrayContaining(["database", "identity"]));
    expect(await readPulled(app)).toEqual(pulled);

    await importPulled(app, pulled);
    const first = await serveLocally(app);
    // The account signs in with the password it had on the origin and reads its own row.
    const there = await member(first.local, hostOf(first), "login");
    expect(await there.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Written on the origin" }]);
    await there.call("actions_invoke", { name: "notes.create", input: { title: "Written locally" } });

    // A later start without --reset keeps data/ as it is.
    const again = await serveLocally(app);
    const back = await member(again.local, hostOf(again), "login");
    expect(await back.call("actions_invoke", { name: "notes.list" })).toHaveLength(2);
    // A second reset imports the pulled data again, after a backup of what data/ held, and the local session survives it.
    await importPulled(app, pulled);
    const backups = (await readdir(path.join(app, ".konae/backups"))).filter((name) => name.endsWith(".json"));
    expect(backups).toHaveLength(2);
    await serveLocally(app);
    expect(await back.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Written on the origin" }]);
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

  test("a folder whose schema.sql is ahead of the exported data runs on it, and the Kernel adds the column with every row kept", async () => {
    const next = path.join(root, "next");
    const born = configOf(await initLocal(next, { name: "Next version" }));
    await provideLocalAppDependencies(next);
    const published = await publishLocal(next, { ...born, url: origin.url }, origin.token);
    await origin.owner.call("signup_policy_set", { deploymentId: published.id, mode: "open" });
    await (await member(origin, hostOf(published), "signup")).call("actions_invoke", { name: "notes.create", input: { title: "Before the column" } });
    const pulled = await pullLocal(next, new McpClient(`${origin.url}/mcp`, { token: origin.token }), origin.url, born.project.id);

    const actions = await readFile(path.join(next, "app", "actions.json"), "utf8");
    expect(actions).toContain("SELECT id, title, created_at FROM notes");
    await writeFile(path.join(next, "app", "actions.json"), actions.replace("SELECT id, title, created_at FROM notes", "SELECT id, title, pinned, created_at FROM notes"));
    const schema = await readFile(path.join(next, "app", "schema.sql"), "utf8");
    expect(schema).toMatch(/created_at[^,\n]*\n\s*\);/);
    await writeFile(path.join(next, "app", "schema.sql"), schema.replace(/(created_at[^,\n]*)\n(\s*\);)/, "$1,\n  pinned INTEGER NOT NULL DEFAULT 0\n$2"));

    await importPulled(next, pulled);
    const released = await serveLocally(next);
    const there = await member(released.local, hostOf(released), "login");
    expect(await there.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Before the column", pinned: 0 }]);
  }, 120_000);
});

/** Every file under `directory` by its path inside it, as bytes. */
async function filesOf(directory: string): Promise<Map<string, Uint8Array>> {
  const files = new Map<string, Uint8Array>();
  for (const entry of await readdir(directory, { recursive: true, withFileTypes: true })) {
    if (!entry.isFile()) continue;
    const full = path.join(entry.parentPath, entry.name);
    files.set(path.relative(directory, full).split(path.sep).join("/"), new Uint8Array(await readFile(full)));
  }
  return files;
}

describe("a file of app/ that is not text", () => {
  test("publish leaves every file of app/ in the installation byte for byte, the export carries them, and a build of the export gives the dist/ the installation serves", async () => {
    const folder = path.join(root, "binary");
    const born = configOf(await initLocal(folder, { name: "Binary folder", template: "tasks" }));
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0xff, 0xfe]);
    await mkdir(path.join(folder, "app/public"), { recursive: true });
    await writeFile(path.join(folder, "app/public/logo.png"), png);
    await writeFile(path.join(folder, "app/public/icon.svg"), '<svg xmlns="http://www.w3.org/2000/svg"/>\n');
    await writeFile(path.join(folder, "app/public/site.webmanifest"), '\ufeff{"name":"Binary folder"}\n');
    await writeFile(path.join(folder, "app/notes.md"), "Kept with the sources.\n");
    const published = await publishLocal(folder, { ...born, url: origin.url }, origin.token, () => {});
    const sources = await filesOf(path.join(folder, "app"));
    expect(await filesOf(path.join(root, "origin", "apps", published.id, "app"))).toEqual(sources);

    const client = new McpClient(`${origin.url}/mcp`, { token: origin.token });
    const exported = await client.call<{ folder: Transfer; downloads: Record<string, Transfer>; sourcesAhead: boolean }>("project_export", { deploymentId: published.id });
    let entries: ReturnType<typeof readTar>;
    try { entries = readTar(await client.download(exported.folder)); }
    finally { for (const transfer of [exported.folder, ...Object.values(exported.downloads)]) await client.cancel(transfer.transferId).catch(() => {}); }
    expect(exported.sourcesAhead).toBe(false);
    const copy = path.join(root, "binary-export");
    for (const entry of entries) {
      await mkdir(path.dirname(path.join(copy, entry.path)), { recursive: true });
      await writeFile(path.join(copy, entry.path), entry.contents);
    }
    expect(await filesOf(path.join(copy, "app"))).toEqual(sources);
    const served = await filesOf(path.join(copy, "dist"));
    expect(served.get("logo.png")).toEqual(png);
    await rm(path.join(copy, "dist"), { recursive: true });
    await buildLocal(copy);
    expect(await filesOf(path.join(copy, "dist"))).toEqual(served);
  }, 60_000);

  test("pull recovers the published sources, and publish refuses a folder behind the observed revision", async () => {
    const folder = path.join(root, "source-roundtrip");
    const settings = configOf(await initLocal(folder, { name: "Source round trip" }));
    await provideLocalAppDependencies(folder);
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0xfe, 0xff]);
    await mkdir(path.join(folder, "app/public"), { recursive: true });
    await writeFile(path.join(folder, "app/public/logo.png"), png);
    const published = await publishLocal(folder, { ...settings, url: origin.url }, origin.token, () => {});
    await origin.owner.call("signup_policy_set", { deploymentId: published.id, mode: "open" });
    const ana = await member(origin, hostOf(published), "signup");
    await ana.call("actions_invoke", { name: "notes.create", input: { title: "Kept through source changes" } });

    const pulled = await pullLocal(folder, new McpClient(`${origin.url}/mcp`, { token: origin.token }), origin.url, settings.project.id);
    expect(pulled.sourcesInstalled).toBe(true);
    expect(pulled.sourcesRevision).toBe(1);
    expect(JSON.parse(await readFile(path.join(folder, ".konae/source-revision.json"), "utf8")).sourcesRevision).toBe(1);
    expect(await readFile(path.join(folder, "app/public/logo.png"))).toEqual(png);
    expect(await readFile(path.join(folder, ".konae/pull/data/database.sqlite"))).toBeInstanceOf(Buffer);

    const schemaPath = path.join(folder, "app/schema.sql");
    const schema = await readFile(schemaPath, "utf8");
    await writeFile(schemaPath, schema.replace(/\n\);\s*$/, ",\n  pinned INTEGER NOT NULL DEFAULT 0\n);"));
    const actionsPath = path.join(folder, "app/actions.json");
    const actions = JSON.parse(await readFile(actionsPath, "utf8"));
    actions.actions["notes.list"].statement = actions.actions["notes.list"].statement.replace("id, title, created_at", "id, title, pinned, created_at");
    await writeFile(actionsPath, JSON.stringify(actions, null, 2));
    const next = await publishLocal(folder, { ...settings, url: origin.url }, origin.token, () => {});
    const nextAccount = await member(origin, hostOf(next), "login");
    expect(await nextAccount.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Kept through source changes", pinned: 0 }]);
    expect(await readFile(path.join(root, "origin/apps", next.id, "app/public/logo.png"))).toEqual(png);

    const inspected = await origin.owner.call<{ project: { name: string; slug: string }; sourcesRevision: number; deployment: { artifactVersion: string } }>("project_inspect", { projectId: settings.project.id });
    const saved = await origin.owner.call<{ revision: number; transfer: Transfer }>("project_sources_read", { projectId: settings.project.id });
    const source = JSON.parse(new TextDecoder().decode(await origin.owner.download(saved.transfer)));
    source.files["remote-change.txt"] = "Saved on the installation";
    const sourceActions = JSON.parse(await readFile(path.join(folder, "app/actions.json"), "utf8"));
    source.manifest = { version: 1, visibility: "private", runtime: { database: { mode: "declared-actions", actions: Object.entries(sourceActions.actions).map(([name, action]: [string, any]) => ({ name, kind: action.kind, access: action.access })) }, auth: { mode: "required" }, files: { access: "deployment" } } };
    const sourceDocument = { ...source, id: settings.project.id, name: inspected.project.name, slug: inspected.project.slug, updatedAt: Date.now() };
    const sourceUpload = await origin.owner.upload(new TextEncoder().encode(JSON.stringify(sourceDocument)), settings.project.id);
    try { await origin.owner.call("project_sources_save", { projectId: settings.project.id, transferId: sourceUpload.transferId, expectedRevision: saved.revision }); }
    finally { await origin.owner.cancel(sourceUpload.transferId); }
    const stalePublish = await publishLocal(folder, { ...settings, url: origin.url }, origin.token, () => {}).then(() => null, (error) => error);
    expect(stalePublish?.refusal?.code).toBe("sources.revision-conflict");
    expect((await origin.owner.call<{ deployment: { artifactVersion: string } }>("project_inspect", { projectId: settings.project.id })).deployment.artifactVersion).toBe(inspected.deployment.artifactVersion);

    const recovered = await pullLocal(folder, new McpClient(`${origin.url}/mcp`, { token: origin.token }), origin.url, settings.project.id);
    expect(recovered.sourcesInstalled).toBe(true);
    expect(await readFile(path.join(folder, "app/remote-change.txt"), "utf8")).toBe("Saved on the installation");
    const { visibility: _visibility, signupPolicy: _signupPolicy, ...pulledManifest } = source.manifest;
    expect(JSON.parse(await readFile(path.join(folder, "konae.json"), "utf8")).manifest).toEqual(pulledManifest);
    const final = await publishLocal(folder, { ...settings, url: origin.url }, origin.token, () => {});
    expect(final.id).toBe(published.id);
    expect(await nextAccount.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Kept through source changes", pinned: 0 }]);
  }, 120_000);

  test("pull stages changed remote settings when the local konae.json has edits, then records them after recovery", async () => {
    const folder = path.join(root, "settings-recovery");
    const settings = configOf(await initLocal(folder, { name: "Settings recovery" }));
    await provideLocalAppDependencies(folder);
    await publishLocal(folder, { ...settings, url: origin.url }, origin.token, () => {});
    const saved = await origin.owner.call<{ revision: number; transfer: Transfer }>("project_sources_read", { projectId: settings.project.id });
    const source = JSON.parse(new TextDecoder().decode(await origin.owner.download(saved.transfer)));
    const actions = JSON.parse(source.files["actions.json"]);
    source.manifest = {
      ...source.manifest,
      visibility: "public",
      connects: ["https://remote.example.test"],
      runtime: {
        database: { mode: "declared-actions", actions: Object.entries(actions.actions).map(([name, action]: [string, any]) => ({ name, kind: action.kind, access: action.access })) },
        auth: { mode: "required" }, files: { access: "deployment" },
      },
    };
    const upload = await origin.owner.upload(new TextEncoder().encode(JSON.stringify(source)), settings.project.id);
    try { await origin.owner.call("project_sources_save", { projectId: settings.project.id, transferId: upload.transferId, expectedRevision: saved.revision }); }
    finally { await origin.owner.cancel(upload.transferId); }

    const localSettings = await readSettings(folder);
    localSettings.manifest.connects = ["https://local.example.test"];
    await writeFile(path.join(folder, "konae.json"), JSON.stringify(localSettings, null, 2) + "\n");
    const client = new McpClient(`${origin.url}/mcp`, { token: origin.token });
    const staged = await pullLocal(folder, client, origin.url, settings.project.id);
    expect(staged.sourcesInstalled).toBe(false);
    expect(JSON.parse(await readFile(path.join(folder, "konae.json"), "utf8")).manifest.connects).toEqual(["https://local.example.test"]);
    expect(JSON.parse(await readFile(path.join(folder, ".konae/pull/konae.json"), "utf8")).manifest.connects).toEqual(["https://remote.example.test"]);
    expect(JSON.parse(await readFile(path.join(folder, ".konae/pull/konae.json"), "utf8")).visibility).toBe("public");
    expect(JSON.parse(await readFile(path.join(folder, ".konae/source-revision.json"), "utf8")).sourcesRevision).toBe(1);

    await rm(path.join(folder, "app"), { recursive: true });
    await cp(path.join(folder, ".konae/pull/app"), path.join(folder, "app"), { recursive: true });
    await cp(path.join(folder, ".konae/pull/konae.json"), path.join(folder, "konae.json"));
    const recovered = await pullLocal(folder, client, origin.url, settings.project.id);
    expect(recovered.sourcesInstalled).toBe(true);
    expect(JSON.parse(await readFile(path.join(folder, ".konae/source-revision.json"), "utf8")).sourcesRevision).toBe(2);
    expect(JSON.parse(await readFile(path.join(folder, "konae.json"), "utf8")).manifest.connects).toEqual(["https://remote.example.test"]);
    expect(JSON.parse(await readFile(path.join(folder, "konae.json"), "utf8")).visibility).toBe("public");
  }, 60_000);

  test("a failure retiring app/ leaves the original app, pull data and revision marker untouched", async () => {
    const folder = path.join(root, "retire-failure");
    const settings = configOf(await initLocal(folder, { name: "Retire failure" }));
    await provideLocalAppDependencies(folder);
    await publishLocal(folder, { ...settings, url: origin.url }, origin.token, () => {});
    const client = new McpClient(`${origin.url}/mcp`, { token: origin.token });
    await pullLocal(folder, client, origin.url, settings.project.id);
    const oldApp = await readFile(path.join(folder, "app/actions.json"));
    const oldSettings = await readFile(path.join(folder, "konae.json"));
    const oldMarker = await readFile(path.join(folder, ".konae/source-revision.json"));
    const oldRecord = await readFile(path.join(folder, ".konae/pull/pull.json"));
    await writeFile(path.join(folder, ".konae/pull/keep.bin"), new Uint8Array([0, 255, 7]));
    const appPath = path.join(folder, "app");
    const operations = { rename: async (from: string, to: string) => {
      if (from === appPath) throw new Error("Injected app retirement failure");
      return rename(from, to);
    } };
    await expect(pullLocal(folder, client, origin.url, settings.project.id, operations)).rejects.toThrow("Injected app retirement failure");
    expect(await readFile(path.join(folder, "app/actions.json"))).toEqual(oldApp);
    expect(await readFile(path.join(folder, "konae.json"))).toEqual(oldSettings);
    expect(await readFile(path.join(folder, ".konae/source-revision.json"))).toEqual(oldMarker);
    expect(await readFile(path.join(folder, ".konae/pull/pull.json"))).toEqual(oldRecord);
    expect(new Uint8Array(await readFile(path.join(folder, ".konae/pull/keep.bin")))).toEqual(new Uint8Array([0, 255, 7]));
  }, 60_000);
});
