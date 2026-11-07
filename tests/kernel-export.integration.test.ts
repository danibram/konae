import { afterAll, beforeAll, expect, test } from "bun:test";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { dataEntries, generatedFiles, type AppSettings, type FolderEntry } from "../src/lib/app-folder";
import type { PackageComponentName } from "../src/lib/portable-package";
import { readTar } from "../src/lib/tar";
import type { Transfer } from "../src/mcp/client";
import { account, appFetch, buildKernel, freshClient, password, release, runtime, startKernel, type Kernel } from "./kernel-harness";

// ADR-0027, step 7: an app exported from Konae with its data runs on the
// kernel binary alone. The Kernel opens the exported folder, which carries
// no record of the installation, an existing app user signs in with the
// password they set on Konae, and an operator command run beside the Kernel
// changes what the running app answers.
const repo = path.resolve(import.meta.dir, "..");
const kernelBinary = path.join(repo, "kernel-rs/target/debug/kernel");
let root: string;
let konae: Kernel | undefined;
let alone: ReturnType<typeof Bun.spawn> | undefined;

const actions = { version: 1, actions: {
  "notes.add": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(owner_id, body) VALUES(?1, ?2)", bindings: [{ source: "actor", field: "id" }, { source: "input", key: "body", type: "string", maxLength: 200 }] },
  "notes.mine": { kind: "query", access: "authenticated", statement: "SELECT body FROM notes WHERE owner_id = ?1 ORDER BY id LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
} };
const schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, owner_id TEXT NOT NULL, body TEXT NOT NULL);";
const payload = (id: string) => ({
  project: { id, name: "Carried", slug: "carried", manifest: { version: 1, visibility: "public" }, files: { "schema.sql": schema, "actions.json": JSON.stringify(actions) } },
  visibility: "public",
  artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Carried</main>" } },
});

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "konae-kernel-export-"));
}, 300_000);

afterAll(async () => {
  await konae?.stop();
  if (alone && alone.exitCode === null) { alone.kill(); await alone.exited; }
  if (root) await rm(root, { recursive: true, force: true });
});

/** The exported folder as the editor's Export writes it: project_export's folder and data, and the files every app folder carries. */
async function exportFolder(kernel: Kernel, deploymentId: string): Promise<FolderEntry[]> {
  const exported = await kernel.owner.call<{ manifest: { contents: { name: PackageComponentName }[] }; downloads: Record<string, Transfer>; folder: Transfer }>("project_export", { deploymentId });
  try {
    const entries = readTar(await kernel.owner.download(exported.folder));
    for (const { name } of exported.manifest.contents) entries.push(...dataEntries(name, await kernel.owner.download(exported.downloads[name])));
    const text = (file: string) => new TextDecoder().decode(entries.find((entry) => entry.path === file)!.contents as Uint8Array);
    const settings = JSON.parse(text("konae.json")) as AppSettings;
    entries.push(...generatedFiles({
      settings, actionsSource: text("app/actions.json"), schemaSource: text("app/schema.sql"),
      runtime: await readFile(path.join(repo, "src/mcp/runtime.js"), "utf8"),
      staticArtifact: await readFile(path.join(repo, "src/lib/static-artifact.js"), "utf8"),
    }));
    return entries;
  } finally {
    for (const transfer of [exported.folder, ...Object.values(exported.downloads)]) await kernel.owner.cancel(transfer.transferId).catch(() => {});
  }
}

test("an app exported with its data runs on the kernel binary, and an operator command reaches the running app", async () => {
  const port = Number(process.env.KONAE_KERNEL_EXPORT_TEST_PORT) || 19700 + Math.floor(Math.random() * 90);
  konae = await startKernel({ port, data: path.join(root, "konae"), setupKey: "kernel-export-setup-secret", ownerEmail: "owner@carried.test" });
  const id = "carried_app";
  await konae.owner.call("project_create", { projectId: id, name: "Carried" });
  const { receipt } = await release(konae.owner, payload(id), id);
  const ana = await account(konae, receipt.hostname, "signup", "ana@carried.test");
  expect(await runtime(konae, receipt.hostname, ana).call("actions_invoke", { name: "notes.add", input: { body: "written on Konae" } })).toMatchObject({ changes: 1 });
  const entries = await exportFolder(konae, receipt.id);
  await konae.stop();
  konae = undefined;

  // The folder goes to a data directory no Konae ever used.
  const data = path.join(root, "kernel");
  const folder = path.join(data, "apps", id);
  for (const entry of entries) {
    await mkdir(path.dirname(path.join(folder, entry.path)), { recursive: true });
    await writeFile(path.join(folder, entry.path), entry.contents);
  }
  expect(existsSync(path.join(folder, ".konae"))).toBe(false);
  expect(existsSync(path.join(folder, "data/identity.sqlite"))).toBe(true);

  const url = `http://127.0.0.1:${port}`;
  const env = {
    ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))) as Record<string, string>,
    KONAE_DATA_DIR: data, KONAE_KERNEL_PORT: String(port), KONAE_BIND: "127.0.0.1", KONAE_PUBLIC_ORIGIN: url,
    KONAE_HOST_SUFFIX: "localhost", KONAE_TRUSTED_PROXIES: "127.0.0.1", KONAE_MAIL_TRANSPORT: "development",
  };
  alone = Bun.spawn([kernelBinary], { env, stdout: "pipe", stderr: "inherit" });
  const served = (async () => {
    let text = "";
    for await (const block of alone!.stdout as ReadableStream<Uint8Array>) { text += new TextDecoder().decode(block); if (text.includes("listening")) return text; }
    throw Error(`The kernel binary stopped during startup:\n${text}`);
  })();
  const startup = await Promise.race([served, Bun.sleep(30_000).then(() => { throw Error("The kernel binary did not start."); })]);
  expect(startup).toContain(`Serving Carried at http://carried.localhost:${port}`);
  // With no KONAE_MAIL_SECRET, the Kernel seals queued links with a key it wrote for itself, readable by its owner alone.
  expect((await stat(path.join(data, "mail-key"))).mode & 0o777).toBe(0o600);
  const hostname = "carried.localhost";
  const app = { url, data } as Kernel;

  // Ana signs in with the password she set on Konae, and reads her row.
  const session = await account(app, hostname, "login", "ana@carried.test");
  const ana2 = runtime(app, hostname, session);
  expect(await ana2.call("actions_invoke", { name: "notes.mine", input: {} })).toEqual([{ body: "written on Konae" }]);
  expect(await ana2.call("actions_invoke", { name: "notes.add", input: { body: "written alone" } })).toMatchObject({ changes: 1 });

  const command = async (...args: string[]) => {
    const process = Bun.spawn([kernelBinary, ...args], { env, stdout: "pipe", stderr: "pipe" });
    const [code, output, error] = await Promise.all([process.exited, new Response(process.stdout).text(), new Response(process.stderr).text()]);
    return { code, output, error };
  };
  const listed = await command("users");
  expect(listed.code).toBe(0);
  const users = JSON.parse(listed.output) as { users: { userId: string; email: string; state: string }[] };
  const user = users.users.find((entry) => entry.email === "ana@carried.test")!;
  expect(user.state).toBe("active");

  // An invitation from the machine leaves through the running Kernel's mail worker.
  const invited = await command("invite", "bea@carried.test");
  expect(invited.error).toBe("");
  expect(JSON.parse(invited.output)).toMatchObject({ results: [{ email: "bea@carried.test", status: "invited", delivery: "queued" }] });
  let mailed = false;
  for (let attempt = 0; attempt < 50 && !mailed; attempt++) {
    mailed = (await readFile(path.join(data, "mailbox.jsonl"), "utf8").catch(() => "")).includes("bea@carried.test");
    if (!mailed) await Bun.sleep(200);
  }
  expect(mailed).toBe(true);

  // An API key appears once, is listed without its secret, and stays listed revoked.
  const created = JSON.parse((await command("api-key", "create", "reader", "--files", "public")).output) as { keyId: string; key: string };
  expect(created.key.length).toBeGreaterThan(20);
  const keys = () => command("api-key", "list").then((listed) => JSON.parse(listed.output) as { keys: { keyId: string; revokedAt: number | null }[] });
  expect((await keys()).keys).toEqual([expect.objectContaining({ keyId: created.keyId, revokedAt: null })]);
  expect(JSON.parse((await command("api-key", "revoke", created.keyId)).output)).toEqual({ keyId: created.keyId, revoked: true });
  expect((await keys()).keys[0].revokedAt).toBeGreaterThan(0);
  expect((await command("api-key", "create", "nothing")).error).toContain("Name at least one action or file visibility for the key.");

  // A suspension from the machine ends the session the running Kernel holds.
  const suspended = await command("suspend", user.userId);
  expect(suspended.error).toBe("");
  expect(suspended.code).toBe(0);
  expect(JSON.parse(suspended.output)).toMatchObject({ userId: user.userId, state: "suspended" });
  await expect(ana2.call("actions_invoke", { name: "notes.mine", input: {} })).rejects.toThrow();
  const again = await appFetch(app, hostname, "/_konae/auth/login", { method: "POST", body: JSON.stringify({ email: "ana@carried.test", password }) });
  expect(again.status).not.toBe(200);
  // Konae's tool refuses a second resume with these words; the command does too.
  expect((await command("resume", user.userId)).code).toBe(0);
  const twice = await command("resume", user.userId);
  expect(twice.code).not.toBe(0);
  expect(twice.error).toContain("This account is not suspended.");

  // The Signup Policy lands in konae.json, and the running Kernel follows it.
  expect(JSON.parse((await command("signup-policy", "closed")).output)).toMatchObject({ mode: "closed", previousMode: "open" });
  expect(JSON.parse(await readFile(path.join(folder, "konae.json"), "utf8")).signupPolicy.mode).toBe("closed");
  let refused = 0;
  for (let attempt = 0; attempt < 30 && refused === 0; attempt++) {
    const response = await appFetch(app, hostname, "/_konae/auth/signup", { method: "POST", headers: { "x-forwarded-for": freshClient() }, body: JSON.stringify({ email: `late${attempt}@carried.test`, password }) });
    if (response.status === 200) await Bun.sleep(200); else refused = response.status;
  }
  expect(refused).toBe(403);
  expect((await command("sql", "SELECT body FROM notes ORDER BY id LIMIT 5")).output).toContain("written alone");

  // An entry link from the machine signs a browser in as Ana, and the link works once.
  const link = JSON.parse((await command("enter", "Ana@Carried.test")).output) as { userId: string; url: string };
  expect(link.userId).toBe(user.userId);
  expect(link.url).toStartWith(`http://${hostname}:${port}/_konae/enter#entry=`);
  const token = new URL(link.url).hash.replace("#entry=", "");
  const enter = () => appFetch(app, hostname, "/_konae/enter", { method: "POST", body: JSON.stringify({ token }), headers: { origin: `http://${hostname}` } });
  const entered = await enter();
  expect(entered.status).toBe(200);
  expect((await enter()).status).toBe(410);
  const asAna = runtime(app, hostname, entered.headers.get("set-cookie")!.split(";")[0]);
  expect(await asAna.call("actions_invoke", { name: "notes.mine", input: {} })).toHaveLength(2);
  expect((await command("enter", "nobody@carried.test")).error).toContain("This app has no account with the email nobody@carried.test.");

  // Maintenance from the machine pauses the running app at once, and off lets it answer again.
  expect(JSON.parse((await command("maintenance", "on")).output)).toMatchObject({ maintenance: true });
  expect((await appFetch(app, hostname, "/")).status).toBe(503);
  await expect(asAna.call("actions_invoke", { name: "notes.mine", input: {} })).rejects.toThrow("maintenance");
  expect(JSON.parse((await command("maintenance", "off")).output)).toMatchObject({ maintenance: false });
  expect((await appFetch(app, hostname, "/")).status).toBe(200);

  // An import from the machine puts the exported data back, after a backup, with the Kernel stopped.
  alone.kill(); await alone.exited;
  const incoming = path.join(root, "incoming");
  for (const entry of entries.filter((entry) => entry.path.startsWith("data/"))) {
    await mkdir(path.dirname(path.join(incoming, entry.path)), { recursive: true });
    await writeFile(path.join(incoming, entry.path), entry.contents);
  }
  const imported = await command("import", incoming);
  expect(imported.error).toBe("");
  expect(JSON.parse(imported.output)).toMatchObject({ imported: true, replace: "data", backup: { holds: ["data"] } });
  expect(JSON.parse((await command("sql", "SELECT body FROM notes ORDER BY id LIMIT 5")).output)).toMatchObject({ rows: [{ body: "written on Konae" }] });
}, 120_000);
