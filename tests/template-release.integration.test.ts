// Every project the editor can start, and each showcase app that carries a
// Database Model, passes release_plan on a fresh Kernel. The Kernel refuses a
// release whose accepted model does not match the schema.sql it deploys, so a
// template whose two files drift apart cannot be published from the editor.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";
import { applyTemplate, templateChoices } from "../src/lib/templates";
import { starterProject } from "../src/lib/starter";

let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let client: McpClient;
const port = Number(process.env.LITTLEBOX_MCP_TEST_PORT) || 19400 + Math.floor(Math.random() * 90);
const url = `http://127.0.0.1:${port}`;
const setupKey = "template-release-setup";
const artifact = { version: 1, entrypoint: "index.html", files: { "index.html": "<main>release</main>" } };

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "littlebox-template-release-"));
  kernel = Bun.spawn(["kernel-rs/target/debug/littlebox-kernel"], {
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_"))),
      LITTLEBOX_DATA_DIR: root, LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_ADMIN_KEY: setupKey,
      LITTLEBOX_PUBLIC_ORIGIN: url, LITTLEBOX_HOST_SUFFIX: "localhost", LITTLEBOX_BUN_PATH: process.execPath, LITTLEBOX_SOURCE_ROOT: process.cwd(),
    },
    stdout: "ignore", stderr: "inherit",
  });
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_littlebox/health`)).ok) break; } catch {}
    await Bun.sleep(100);
  }
  const signup = await fetch(`${url}/_littlebox/owner/signup`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email: "owner@templates.local", password: setupKey, setupKey }) });
  expect(signup.ok).toBe(true);
  const cookie = signup.headers.get("set-cookie")!.split(";")[0];
  client = new McpClient(`${url}/mcp`, { fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }) });
}, 180_000);

afterAll(async () => {
  kernel?.kill();
  if (kernel) await kernel.exited;
  if (root) await rm(root, { recursive: true, force: true });
});

type Release = { id: string; files: Record<string, string>; manifest: object; model: string };

async function plan({ id, files, manifest, model }: Release) {
  await client.call("project_create", { projectId: id, name: id, sourceAuthority: "managed", manifest });
  const program = JSON.parse(files["actions.json"]) as { actions: Record<string, { http?: { signature?: { secret?: string } }; auth?: { secret?: string } }> };
  for (const action of Object.values(program.actions)) {
    const secret = action.http?.signature?.secret ?? action.auth?.secret;
    if (secret) await client.call("secret_set", { projectId: id, name: secret, value: `whsec_${btoa(`${id}-${secret}-signing-key`)}` });
  }
  const databaseModel = { source: model, acceptedSource: model, revision: "baseline" };
  const project = { id, name: id, slug: id.replaceAll("_", "-"), files, manifest, databaseModel };
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify({ project, visibility: "private", artifact })), id);
  try { return await client.call<{ planId: string }>("release_plan", { transferId: transfer.transferId }); }
  finally { await client.cancel(transfer.transferId).catch(() => {}); }
}

for (const { id } of templateChoices) {
  test(`the ${id} template plans a release with its accepted model`, async () => {
    const project = applyTemplate({ ...structuredClone(starterProject), id: `template_${id}`, slug: `template-${id}` }, id);
    const files = Object.fromEntries(Object.entries(project.files).filter((entry): entry is [string, string] => typeof entry[1] === "string"));
    const manifest = { ...project.manifest, visibility: "private" };
    expect((await plan({ id: project.id, files, manifest, model: project.databaseModel.source })).planId).toBeTruthy();
  }, 60_000);
}

for (const app of ["habits", "starter"]) {
  test(`the ${app} showcase plans a release with schema.ts as its accepted model`, async () => {
    const dir = path.join("examples/showcase", app);
    const files: Record<string, string> = {};
    for (const name of ["schema.sql", "schema.ts", "actions.json"]) files[name] = await readFile(path.join(dir, name), "utf8");
    const manifest = { version: 1, visibility: "private" };
    expect((await plan({ id: `showcase_${app}`, files, manifest, model: files["schema.ts"] })).planId).toBeTruthy();
  }, 60_000);
}
