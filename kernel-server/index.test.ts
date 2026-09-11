import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import path from "node:path";
import { tmpdir } from "node:os";
import { cloneStarter } from "../src/lib/starter";

const adminKey = "kernel-integration-test-key";
let port = 0;
let origin = "";
let dataRoot = "";
let processHandle: ReturnType<typeof Bun.spawn> | undefined;

beforeAll(async () => {
  dataRoot = await mkdtemp(path.join(tmpdir(), "littlebox-kernel-test-"));
  port = 20_000 + (process.pid % 20_000);
  origin = `http://127.0.0.1:${port}`;
  processHandle = Bun.spawn(["bun", "run", "kernel-server/index.ts"], {
    cwd: path.resolve(import.meta.dir, ".."),
    env: {
      ...process.env,
      LITTLEBOX_KERNEL_PORT: String(port),
      LITTLEBOX_DATA_DIR: dataRoot,
      LITTLEBOX_ADMIN_KEY: adminKey,
      LITTLEBOX_PUBLIC_ORIGIN: `http://localhost:${port}`,
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  for (let attempt = 0; attempt < 80; attempt += 1) {
    try {
      if ((await fetch(`${origin}/_littlebox/health`)).ok) return;
    } catch {
      // The Kernel process may still be opening SQLite.
    }
    await Bun.sleep(25);
  }
  if (processHandle.exitCode !== null) {
    const stderr = await new Response(processHandle.stderr).text();
    const stdout = await new Response(processHandle.stdout).text();
    throw new Error(`Kernel exited with ${processHandle.exitCode}.\n${stdout}\n${stderr}`);
  }
  throw new Error("Kernel integration server stayed alive but did not become ready.");
});

afterAll(async () => {
  processHandle?.kill();
  await processHandle?.exited;
  if (dataRoot) await rm(dataRoot, { recursive: true, force: true });
});

describe("Littlebox Kernel HTTP contract", () => {
  test("deploys isolated public and private apps without exposing database administration", async () => {
    const publicDeployment = await deploy("public");
    const publicHeaders = { "x-forwarded-host": publicDeployment.hostname };

    expect((await fetch(`${origin}/`, { headers: publicHeaders })).status).toBe(200);
    expect(await action(publicDeployment.hostname, "tasks.create", { title: "Persisted task" })).toEqual({ changes: 1, lastInsertId: 1 });
    expect(await action(publicDeployment.hostname, "tasks.list", {})).toMatchObject([{ id: 1, title: "Persisted task" }]);

    const form = new FormData();
    form.append("file", new File(["stored by the kernel"], "proof.txt", { type: "text/plain" }));
    const upload = await fetch(`${origin}/_littlebox/files`, { method: "POST", headers: publicHeaders, body: form });
    expect(upload.status).toBe(201);
    const uploaded = (await upload.json() as { result: { id: string } }).result;
    expect((await fetch(`${origin}/_littlebox/files/${uploaded.id}`, { headers: publicHeaders })).status).toBe(200);

    const adminUrl = `${origin}/_littlebox/admin/deployments/${publicDeployment.id}/database`;
    expect((await fetch(adminUrl, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ operation: "inspect" }) })).status).toBe(401);
    const inspect = await fetch(adminUrl, {
      method: "POST",
      headers: { "content-type": "application/json", "x-littlebox-admin-key": adminKey },
      body: JSON.stringify({ operation: "inspect" }),
    });
    expect(await inspect.json()).toMatchObject({ result: [{ name: "tasks", totalRows: 1 }] });

    const privateDeployment = await deploy("private");
    const privateHeaders = { "x-forwarded-host": privateDeployment.hostname };
    expect((await fetch(`${origin}/`, { headers: privateHeaders })).status).toBe(401);
    expect((await fetch(`${origin}/_littlebox/actions/tasks.list`, {
      method: "POST",
      headers: { ...privateHeaders, "content-type": "application/json" },
      body: "{}",
    })).status).toBe(401);

    const login = await fetch(`${origin}/_littlebox/login`, {
      method: "POST",
      redirect: "manual",
      headers: privateHeaders,
      body: new URLSearchParams({ code: privateDeployment.accessCode }),
    });
    expect(login.status).toBe(303);
    const cookie = login.headers.get("set-cookie")?.split(";", 1)[0];
    expect(cookie).toStartWith("littlebox_session=");
    expect((await fetch(`${origin}/`, { headers: { ...privateHeaders, cookie: cookie ?? "" } })).status).toBe(200);
  }, 15_000);

  test("deploys a static app without schema or server actions", async () => {
    const project = cloneStarter();
    project.id = "app_static";
    project.name = "Static architecture";
    project.slug = "static-architecture";
    project.files["schema.sql"] = "";
    project.files["actions.json"] = JSON.stringify({ version: 1, actions: {} });
    project.manifest.runtime.database.actions = [];

    const deployment = await deploy("public", project);
    const response = await fetch(`${origin}/`, { headers: { "x-forwarded-host": deployment.hostname } });

    expect(response.status).toBe(200);
    expect(await response.text()).toContain("MY QUIET LIST");
  });
});

async function deploy(visibility: "public" | "private", project = cloneStarter()) {
  const response = await fetch(`${origin}/_littlebox/admin/deploy`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-littlebox-admin-key": adminKey },
    body: JSON.stringify({ project, visibility }),
  });
  expect(response.status).toBe(201);
  const result = (await response.json() as { result: { id: string; url: string; accessCode: string } }).result;
  return { ...result, hostname: new URL(result.url).hostname };
}

async function action(hostname: string, name: string, input: unknown) {
  const response = await fetch(`${origin}/_littlebox/actions/${encodeURIComponent(name)}`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-forwarded-host": hostname },
    body: JSON.stringify(input),
  });
  expect(response.status).toBe(200);
  return (await response.json() as { result: unknown }).result;
}
