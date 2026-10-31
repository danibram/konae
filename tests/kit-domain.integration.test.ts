import { afterAll, beforeAll, expect, spyOn, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildProjectExport } from "../src/lib/export-project-kit";
import { createDefaultManifest } from "../src/deployments/manifest";
import { HostedDatabaseClient, type PortableProjectState } from "../src/deployments/client";
import { McpClient, platformMcp } from "../src/mcp/client";

let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let client: McpClient;
let url: string;
let cookie: string;
const schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL);";
const manifest = { ...createDefaultManifest(), visibility: "private" as const };
const files = { "schema.sql": schema, "actions.json": '{"version":1,"actions":{}}' };

async function start() {
  kernel = Bun.spawn(["kernel-rs/target/debug/konae-kernel"], {
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))), KONAE_DATA_DIR: root, KONAE_KERNEL_PORT: new URL(url).port, KONAE_BIND: "127.0.0.1", KONAE_ADMIN_KEY: "review-test-setup", KONAE_PUBLIC_ORIGIN: url, KONAE_HOST_SUFFIX: "localhost" },
    stdout: "ignore", stderr: "inherit",
  });
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(url + "/_konae/health")).ok) return; } catch {}
    await Bun.sleep(100);
  }
  throw Error("Test Kernel did not start.");
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "konae-review-"));
  url = `http://127.0.0.1:${21000 + Math.floor(Math.random() * 1000)}`;
  await start();
  const signup = await fetch(url + "/_konae/owner/signup", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email: "owner@selfhost.local", password: "review-test-setup", setupKey: "review-test-setup" }) });
  expect(signup.ok).toBe(true);
  cookie = signup.headers.get("set-cookie")!.split(";")[0];
  client = new McpClient(url + "/mcp", { fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }) });
}, 120_000);

afterAll(async () => { kernel?.kill(); if (kernel) await kernel.exited; if (root) await rm(root, { recursive: true, force: true }); });

async function publish(id: string, initialDomain?: string) {
  await client.call("project_create", { projectId: id, name: id, manifest });
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify({ project: { id, name: id, slug: id, files, manifest }, visibility: "private", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<main>test</main>" } }, ...(initialDomain ? { initialDomain } : {}) })), id);
  try {
    const plan = await client.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    return await client.call<{ id: string }>("release_apply", { planId: plan.planId });
  } finally { await client.cancel(transfer.transferId); }
}

async function inspect(id: string) {
  return (await client.call<{ deployment: { id: string } }>("project_inspect", { projectId: id })).deployment;
}

test("portable export carries only the domain name and an import remains pending", async () => {
  const origin = await publish("domain_origin", "notes.example.org");
  const domain = await client.call<{ record: { value: string } }>("domain_read", { deploymentId: origin.id });
  const transfer = await client.call<Parameters<McpClient["download"]>[0]>("project_export", { deploymentId: origin.id });
  const exported = new TextDecoder().decode(await client.download(transfer));
  const state = JSON.parse(exported).result;
  expect(state.domain).toBe("notes.example.org");
  expect(exported).not.toContain(domain.record.value);
  expect(state.visibility).toBe("private");
  await client.call("domain_clear", { deploymentId: origin.id });
  const imported = await publish("domain_destination", state.domain);
  const pending = await client.call<{ status: string; record: { value: string } }>("domain_read", { deploymentId: imported.id });
  expect(pending.status).toBe("pending");
  expect(pending.record.value).not.toBe(domain.record.value);
  const persisted = JSON.parse(await readFile(path.join(root, "deployments", imported.id, "domain.json"), "utf8"));
  expect(persisted.verified).toBe(false);
});

test("the generated editor kit imports private data and prints the destination TXT record on repeated deploys", async () => {
  const origin = await publish("kit_origin", "kit.example.org");
  await client.call("database_execute", { deploymentId: origin.id, operation: "sql", input: { sql: "INSERT INTO notes(title) VALUES('portable row')" } });
  const transport = spyOn(platformMcp, "call").mockImplementation((name, args) => client.call(name, args));
  let state: PortableProjectState;
  try { state = await new HostedDatabaseClient(origin.id).exportPortableState(); }
  finally { transport.mockRestore(); }
  expect(state.domain).toBe("kit.example.org");
  const sourceDomain = await client.call<{ record: { value: string } }>("domain_read", { deploymentId: origin.id });
  await client.call("domain_clear", { deploymentId: origin.id });
  const directory = await mkdtemp(path.join(tmpdir(), "konae-kit-domain-"));
  try {
    const kit = buildProjectExport({ id: "kit_destination", name: "Kit", slug: "kit", manifest: { ...manifest, visibility: "public" }, files, directories: [], updatedAt: 1 },
      { version: 1, entrypoint: "index.html", files: { "index.html": "<script data-konae-runtime>window.miniapp={}</script>" } }, [],
      state);
    for (const entry of kit.entries) {
      const target = path.join(directory, entry.path);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, entry.contents);
    }
    await writeFile(path.join(directory, ".env"), `KONAE_PORT=${new URL(url).port}\nKONAE_ADMIN_KEY=review-test-setup\n`);
    for (let run = 0; run < 2; run++) {
      const child = Bun.spawn(["node", "scripts/deploy.mjs"], { cwd: directory, stdout: "pipe", stderr: "pipe" });
      const [code, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
      expect(stderr).toBe("");
      expect(code).toBe(0);
      const destination = await inspect("kit_destination");
      const domain = await client.call<{ status: string; record: { name: string; value: string } }>("domain_read", { deploymentId: destination.id });
      expect(domain.status).toBe("pending");
      expect(domain.record.value).not.toBe(sourceDomain.record.value);
      expect(stdout).toContain(domain.record.value);
      expect(stdout).toContain("set KONAE_APP_DOMAIN=kit.example.org in .env");
      const policy = await client.call<{ visibility: string }>("signup_policy_read", { deploymentId: destination.id });
      expect(policy.visibility).toBe("private");
      const data = await client.call<{ rows: { title: string }[] }>("database_execute", { deploymentId: destination.id, operation: "sql", input: { sql: "SELECT title FROM notes LIMIT 1" } });
      expect(data.rows[0].title).toBe("portable row");
    }
  } finally { await rm(directory, { recursive: true, force: true }); }
});
