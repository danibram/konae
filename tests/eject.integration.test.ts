import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";
import { checkEjectPackage, type EjectPlan, type EjectReceipt } from "../src/lib/eject-package";

// Delivery I: Eject on the server, and every refusal its row in
// docs/plans/platform-launch.md lists.
let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let url: string;
let owner: McpClient;
const setupKey = "eject-integration-setup-secret";
const password = "eject-integration-password";
const closed = "This project was ejected";
let nextClient = 0;
const freshClient = () => { nextClient++; return `198.20.${Math.floor(nextClient / 250)}.${nextClient % 250 + 1}`; };

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "konae-eject-"));
  const port = Number(process.env.KONAE_EJECT_TEST_PORT) || 19700 + Math.floor(Math.random() * 90);
  url = `http://127.0.0.1:${port}`;
  kernel = Bun.spawn(["kernel-rs/target/debug/konae-kernel"], {
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))), KONAE_DATA_DIR: path.join(root, "data"), KONAE_KERNEL_PORT: String(port), KONAE_BIND: "127.0.0.1", KONAE_ADMIN_KEY: setupKey, KONAE_PUBLIC_ORIGIN: url, KONAE_HOST_SUFFIX: "localhost", KONAE_BUN_PATH: process.execPath, KONAE_TRUSTED_PROXIES: "127.0.0.1", KONAE_MAIL_TRANSPORT: "development" },
    stdout: "ignore", stderr: "inherit",
  });
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_konae/health`)).ok) break; } catch {}
    if (kernel.exitCode !== null) throw Error("The eject test Kernel stopped.");
    await Bun.sleep(100);
  }
  const response = await fetch(`${url}/_konae/owner/signup`, { method: "POST", headers: { "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email: "owner@eject.test", password, setupKey }) });
  expect(response.status).toBe(200);
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}, 300_000);

afterAll(async () => { kernel?.kill(); if (kernel) await kernel.exited; if (root) await rm(root, { recursive: true, force: true }); });

const files = {
  "index.html": "<main id=\"app\"></main>",
  "app.js": "document.querySelector('#app').textContent = 'Eject proof';\n",
  "styles.css": "body { font: 16px system-ui; }",
  "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, title TEXT NOT NULL, user_id TEXT NOT NULL);",
  "actions.json": JSON.stringify({ version: 1, actions: {
    "notes.list": { kind: "query", access: "authenticated", statement: "SELECT id,title FROM notes WHERE user_id=?1 ORDER BY id LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
    "notes.create": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(title,user_id) VALUES(?1,?2)", bindings: [{ source: "input", key: "title", type: "string", maxLength: 100 }, { source: "actor", field: "id" }] },
  } }),
};

function source(id: string, extra: Record<string, string> = {}) {
  return {
    id, name: "Eject proof", slug: id.replaceAll("_", "-"), files: { ...files, ...extra }, directories: [],
    databaseModel: { source: "export const notes = {};\n", acceptedSource: "export const notes = {};\n", revision: "r1" },
    manifest: { version: 1, visibility: "public", runtime: { database: { mode: "declared-actions", actions: [{ name: "notes.list", kind: "query", access: "authenticated" }, { name: "notes.create", kind: "mutation", access: "authenticated" }] }, auth: { mode: "optional" }, files: { access: "deployment" } } },
    updatedAt: 1,
  };
}

/** One save from an editor tab that believes the sources are at `expectedRevision`. */
async function save(client: McpClient, id: string, expectedRevision: number, extra: Record<string, string> = {}) {
  const upload = await client.upload(new TextEncoder().encode(JSON.stringify(source(id, extra))), id);
  try { return await client.call<{ revision: number }>("project_sources_save", { projectId: id, transferId: upload.transferId, expectedRevision }); }
  finally { await client.cancel(upload.transferId).catch(() => {}); }
}

async function release(client: McpClient, id: string, page = "Eject proof") {
  const payload = { project: { id, name: "Eject proof", slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility: "public" }, files }, visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": `<!doctype html><main>${page}</main>` } } };
  const upload = await client.upload(new TextEncoder().encode(JSON.stringify(payload)), id);
  try {
    const plan = await client.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
    return await client.call<{ id: string; hostname: string; url: string }>("release_apply", { planId: plan.planId });
  } finally { await client.cancel(upload.transferId).catch(() => {}); }
}

async function managed(id: string) {
  await owner.call("project_create", { projectId: id, name: "Eject proof", sourceAuthority: "managed" });
  expect(await save(owner, id, 0)).toEqual({ revision: 1 });
}

async function delegate(scopes: string[], projectIds: string[] = []) {
  const { token } = await owner.call<{ token: string }>("token_create", { scopes, projectIds, ttlSeconds: 600 });
  return new McpClient(`${url}/mcp`, { token });
}

function runtime(hostname: string, cookie?: string) {
  return new McpClient(`${url}/_konae/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", hostname);
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

function auditRows(tool: string, target: string) {
  const db = new Database(path.join(root, "data/mcp.sqlite"));
  try { return db.query("SELECT ok, detail FROM audit WHERE tool=?1 AND target=?2 ORDER BY id").all(tool, target) as { ok: number; detail: string | null }[]; }
  finally { db.close(); }
}

function transferCount() {
  const db = new Database(path.join(root, "data/mcp.sqlite"));
  try { return (db.query("SELECT COUNT(*) AS n FROM transfers").get() as { n: number }).n; }
  finally { db.close(); }
}

async function plan(client: McpClient, projectId: string, expectedRevision: number) {
  const value = await client.call<EjectPlan>("eject_plan", { projectId, expectedRevision });
  const bytes = await client.download(value.transfer);
  return { plan: value, package: await checkEjectPackage(bytes, value) };
}

describe("Eject on the server", () => {
  test("Eject keeps the Deployment serving, keeps an app user signed in, and refuses a Managed write from an old tab", async () => {
    const id = "eject_serving";
    await managed(id);
    const deployment = await release(owner, id);
    const signup = await fetch(`${url}/_konae/auth/signup`, { method: "POST", headers: { host: deployment.hostname, "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email: "member@eject.test", password }) });
    expect(signup.status).toBe(200);
    const member = runtime(deployment.hostname, signup.headers.get("set-cookie")!.split(";")[0]);
    await member.call("actions_invoke", { name: "notes.create", input: { title: "Written before Eject" } });
    const before = await member.call<{ id: string }>("auth_session");

    // Two tabs: one saves revision 2, the other still believes revision 1.
    expect(await save(owner, id, 1)).toEqual({ revision: 2 });
    expect((await owner.call<{ sourcesRevision: number }>("project_inspect", { projectId: id })).sourcesRevision).toBe(2);
    const { plan: planned, package: code } = await plan(owner, id, 2);
    expect(planned.applied).toBe(false);
    expect(planned.effects.hostedApp).toBe("Keeps URL, users, sessions, data, invitations and email service.");
    expect(Object.keys(code.files).sort()).toEqual(["README.md", "actions.json", "app.js", "index.html", "konae.json", "schema.sql", "schema.ts", "styles.css"]);
    expect(JSON.parse(code.files["konae.json"])).toMatchObject({ url, project: { id, sourceAuthority: "external", manifest: { visibility: "public" } } });
    expect(code.files["README.md"]).toContain("bun run konae -- publish");

    const receipt = await owner.call<EjectReceipt>("eject_apply", { planId: planned.planId });
    expect(receipt).toMatchObject({ projectId: id, sourceAuthority: "external", sourcesRevision: 2, packageSha256: planned.package.sha256, via: "session", deployment: { id: deployment.id, url: deployment.url } });

    // The tab that saved last and the stale one both get the same refusal.
    await expect(save(owner, id, 2, { "app.js": "changed after Eject" })).rejects.toThrow(closed);
    await expect(save(owner, id, 1, { "app.js": "from an old tab" })).rejects.toThrow(closed);
    // An agent with its own write credential gets it too.
    const agent = await delegate(["read", "write", "publish"], [id]);
    await expect(save(agent, id, 2)).rejects.toThrow(closed);
    // Taking the project back into the editor is refused with the way out.
    await expect(owner.call("project_create", { projectId: id, name: "Eject proof", sourceAuthority: "managed" })).rejects.toThrow(closed);

    // The frozen sources stay readable for the read-only editor.
    const frozen = await owner.call<{ revision: number; readOnly: boolean; transfer: Parameters<McpClient["download"]>[0] }>("project_sources_read", { projectId: id });
    expect(frozen).toMatchObject({ revision: 2, readOnly: true });
    expect(JSON.parse(new TextDecoder().decode(await owner.download(frozen.transfer))).files["app.js"]).toBe(files["app.js"]);

    // The hosted app, its user and that user's session continue.
    expect(await member.call<{ id: string }>("auth_session")).toEqual(before);
    expect(await member.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Written before Eject" }]);
    const page = await fetch(`${url}/`, { headers: { host: deployment.hostname } });
    expect(page.status).toBe(200);
    expect(await page.text()).toContain("Eject proof");

    // The owner keeps publishing artifacts built outside Konae.
    const inspected = await owner.call<{ revision: number; project: { sourceAuthority: string; ejected: { by: string; sourcesRevision: number } } }>("project_inspect", { projectId: id });
    const republished = await release(agent, id, "Built outside");
    expect(republished.id).toBe(deployment.id);
    expect(republished.hostname).toBe(deployment.hostname);
    expect(await (await fetch(`${url}/`, { headers: { host: deployment.hostname } })).text()).toContain("Built outside");
    const after = await owner.call<typeof inspected>("project_inspect", { projectId: id });
    expect(after.revision).toBe(inspected.revision + 1);
    expect(after.project.sourceAuthority).toBe("external");
    expect(after.project.ejected.sourcesRevision).toBe(2);
    expect(await member.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Written before Eject" }]);
    const listed = await owner.call<{ projects: { project: { id: string; sourceAuthority: string } }[] }>("projects_list");
    expect(listed.projects.find((item) => item.project.id === id)?.project.sourceAuthority).toBe("external");

    expect(auditRows("eject_plan", id).some((row) => row.ok === 1)).toBe(true);
    expect(auditRows("eject_apply", id)).toEqual([{ ok: 1, detail: "sources revision 2" }]);
    expect(auditRows("project_sources_save", id).filter((row) => row.ok === 0).length).toBeGreaterThanOrEqual(3);
  }, 60_000);

  test("Eject with a stale revision is refused before anything downloads", async () => {
    const id = "eject_stale";
    await managed(id);
    const transfers = transferCount();
    await expect(owner.call("eject_plan", { projectId: id, expectedRevision: 0 })).rejects.toThrow("Source revision changed");
    expect(transferCount()).toBe(transfers);
    expect(auditRows("eject_plan", id)).toEqual([{ ok: 0, detail: null }]);
    // A save between plan and apply makes the plan stale, and apply refuses it.
    const { plan: planned } = await plan(owner, id, 1);
    expect(await save(owner, id, 1)).toEqual({ revision: 2 });
    await expect(owner.call("eject_apply", { planId: planned.planId })).rejects.toThrow("Source revision changed since this plan");
    // Nothing changed: the project is still Managed and accepts saves.
    expect((await owner.call<{ project: { sourceAuthority: string } }>("project_inspect", { projectId: id })).project.sourceAuthority).toBe("managed");
    expect(await save(owner, id, 2)).toEqual({ revision: 3 });
  });

  test("Eject needs manage_lifecycle, which only the owner session holds unless delegated", async () => {
    const id = "eject_permission";
    await managed(id);
    await managed("eject_permission_other");
    const session = await owner.call<{ scopes: string[] }>("session_inspect");
    expect(session.scopes).toContain("manage_lifecycle");
    for (const scopes of [["read", "write", "publish"], ["read", "manage_users", "export_data"]]) {
      const client = await delegate(scopes, [id]);
      await expect(client.call("eject_plan", { projectId: id, expectedRevision: 1 })).rejects.toThrow("Unknown tool or insufficient permission");
      await expect(client.call("eject_apply", { planId: "anything" })).rejects.toThrow("Unknown tool or insufficient permission");
    }
    const elsewhere = await delegate(["read", "manage_lifecycle"], ["eject_permission_other"]);
    await expect(elsewhere.call("eject_plan", { projectId: id, expectedRevision: 1 })).rejects.toThrow("outside this token's grants");
    const lifecycle = await delegate(["read", "manage_lifecycle"], [id]);
    const { plan: planned } = await plan(lifecycle, id, 1);
    // A plan belongs to the owner, and a credential limited to another project cannot apply it.
    await expect(elsewhere.call("eject_apply", { planId: planned.planId })).rejects.toThrow("outside this token's grants");
    expect(await lifecycle.call<EjectReceipt>("eject_apply", { planId: planned.planId })).toMatchObject({ projectId: id, via: "token" });
  });

  test("an applied plan reapplied returns its receipt, and the package downloads again", async () => {
    const id = "eject_idempotent";
    await managed(id);
    const first = await plan(owner, id, 1);
    const retried = await plan(owner, id, 1);
    expect(retried.plan.planId).toBe(first.plan.planId);
    expect(retried.plan.package.sha256).toBe(first.plan.package.sha256);
    const receipt = await owner.call<EjectReceipt>("eject_apply", { planId: first.plan.planId });
    expect(await owner.call<EjectReceipt>("eject_apply", { planId: first.plan.planId })).toEqual(receipt);
    // After an interruption the client plans again at the same revision and
    // gets the applied plan, its receipt and the same package.
    const again = await plan(owner, id, 1);
    expect(again.plan).toMatchObject({ planId: first.plan.planId, applied: true, receipt });
    expect(again.package).toEqual(first.package);
    await expect(owner.call("eject_plan", { projectId: id, expectedRevision: 0 })).rejects.toThrow("ejected at sources revision 1");
    expect(auditRows("eject_apply", id).map((row) => row.ok)).toEqual([1, 1]);
  });

  test("a Managed project with no saved sources has nothing to eject", async () => {
    const id = "eject_empty";
    await owner.call("project_create", { projectId: id, name: "Empty", sourceAuthority: "managed" });
    await expect(owner.call("eject_plan", { projectId: id, expectedRevision: 0 })).rejects.toThrow("no saved sources to eject");
  });

  test("a project created outside the editor is already External and does not pass through Eject", async () => {
    const id = "eject_external";
    await owner.call("project_create", { projectId: id, name: "External" });
    await expect(owner.call("eject_plan", { projectId: id, expectedRevision: 0 })).rejects.toThrow("already External");
    await expect(owner.call("project_create", { projectId: id, name: "External", sourceAuthority: "managed" })).rejects.toThrow("This project is External. Konae does not take source authority back, so create a new project");
  });

  test("the CLI refuses a stale revision before downloading, then writes, verifies and applies", async () => {
    const id = "eject_cli";
    await managed(id);
    await release(owner, id);
    const { token } = await owner.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish", "manage_lifecycle"], projectIds: [id], ttlSeconds: 600 });
    const directory = path.join(root, "ejected");
    const run = async (args: string[]) => {
      const child = Bun.spawn([process.execPath, "run", "scripts/konae.ts", ...args], { env: { ...process.env, KONAE_MCP_TOKEN: token }, stdout: "pipe", stderr: "pipe" });
      const [code, output, error] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
      return { code, output, error };
    };
    const stale = await run(["eject", "--dir", directory, "--project", id, "--url", url, "--revision", "0"]);
    expect(stale.code).toBe(1);
    expect(stale.error).toContain("Source revision changed");
    expect(await readdir(directory).catch(() => [])).toEqual([]);
    const ejected = await run(["eject", "--dir", directory, "--project", id, "--url", url]);
    expect(ejected.error).toBe("");
    expect(ejected.code).toBe(0);
    expect(ejected.output).toContain("publish");
    expect(await readFile(path.join(directory, "app.js"), "utf8")).toBe(files["app.js"]);
    expect(JSON.parse(await readFile(path.join(directory, "konae.json"), "utf8")).project.sourceAuthority).toBe("external");
    expect(JSON.parse(await readFile(path.join(directory, ".konae/eject-receipt.json"), "utf8"))).toMatchObject({ projectId: id, sourcesRevision: 1 });
    expect((await owner.call<{ project: { sourceAuthority: string } }>("project_inspect", { projectId: id })).project.sourceAuthority).toBe("external");
    // The folder publishes on its own with the same credential.
    const published = await run(["publish", "--dir", directory]);
    expect(published.code).toBe(0);
    expect(published.output).toContain(".localhost");
    expect(await (await fetch(`${url}/`, { headers: { host: new URL(published.output.trim()).host.split(":")[0] } })).text()).toContain("Eject proof");
  }, 60_000);
});
