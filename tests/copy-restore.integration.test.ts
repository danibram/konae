import { beforeAll, describe, expect, test } from "bun:test";
import { mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient, type Transfer } from "../src/mcp/client";

// Delivery 2's condition: a Kernel started on an empty volume from a copy
// serves the same app with its artifact, rows, accounts and files.
const binary = "kernel-rs/target/debug/konae";
const setupKey = "copy-restore-setup-secret";
const password = "copy-restore-password-2026";
const attachment = new TextEncoder().encode("Attachment survives the copy");
const s3Endpoint = process.env.KONAE_TEST_S3_ENDPOINT ?? "http://127.0.0.1:9000";
const s3Bucket = process.env.KONAE_TEST_S3_BUCKET ?? "konae";
const s3Env = {
  KONAE_S3_ENDPOINT: s3Endpoint,
  KONAE_S3_ACCESS_KEY: process.env.KONAE_TEST_S3_ACCESS_KEY ?? "konae",
  KONAE_S3_SECRET_KEY: process.env.KONAE_TEST_S3_SECRET_KEY ?? "konae-secret",
  KONAE_S3_BUCKET: s3Bucket,
};
const minio = await fetch(`${s3Endpoint}/minio/health/live`, { signal: AbortSignal.timeout(1000) }).then((response) => response.ok, () => false);
if (!minio) console.warn(`MinIO is not reachable at ${s3Endpoint}; the S3 copy target test is skipped.`);

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
}, 300_000);

function kernelEnv(root: string, extra: Record<string, string> = {}) {
  const inherited = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_")));
  return { ...inherited, KONAE_DATA_DIR: root, KONAE_BIND: "127.0.0.1", KONAE_SETUP_KEY: setupKey, KONAE_HOST_SUFFIX: "localhost", ...extra };
}

async function startKernel(root: string, extra: Record<string, string> = {}) {
  const port = 19500 + Math.floor(Math.random() * 400);
  const url = `http://127.0.0.1:${port}`;
  const child = Bun.spawn([binary], { env: kernelEnv(root, { KONAE_KERNEL_PORT: String(port), KONAE_PUBLIC_ORIGIN: url, ...extra }), stdout: "ignore", stderr: "inherit" });
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_konae/health`)).ok) return { url, child }; } catch {}
    if (child.exitCode !== null) throw Error("The Kernel stopped during startup.");
    await Bun.sleep(100);
  }
  child.kill();
  throw Error("The Kernel did not start.");
}

async function stop(child: ReturnType<typeof Bun.spawn>) {
  child.kill();
  await child.exited;
}

async function restore(from: string, root: string, extra: Record<string, string> = {}, copy?: string) {
  const child = Bun.spawn([binary, "restore", "--from", from, ...(copy ? ["--copy", copy] : [])], { env: kernelEnv(root, extra), stdout: "pipe", stderr: "pipe" });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  return { status, output: stdout + stderr };
}

function withHeaders(endpoint: string, headers: Record<string, string>) {
  return new McpClient(endpoint, { fetch: (input, init) => {
    const merged = new Headers(init?.headers);
    for (const [name, value] of Object.entries(headers)) merged.set(name, value);
    return fetch(input, { ...init, headers: merged });
  } });
}

async function sign(url: string, route: string, body: object, host?: string) {
  const response = await fetch(`${url}${route}`, { method: "POST", headers: { "content-type": "application/json", ...(host ? { host } : {}) }, body: JSON.stringify(body) });
  expect(response.status).toBe(200);
  return response.headers.get("set-cookie")!.split(";")[0];
}

const project = {
  "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, title TEXT NOT NULL, user_id TEXT NOT NULL);",
  "actions.json": JSON.stringify({ version: 1, actions: {
    "notes.list": { kind: "query", access: "authenticated", statement: "SELECT id,title FROM notes WHERE user_id=?1 LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
    "notes.create": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(title,user_id) VALUES(?1,?2)", bindings: [{ source: "input", key: "title", type: "string", maxLength: 100 }, { source: "actor", field: "id" }] },
  } }),
};

/** Publishes an app on a Kernel that copies to `copyEnv`, writes a row, an account and a file, waits for a copy that started after those writes, and returns what the second Kernel must serve. */
async function publishAndCopy(root: string, copyEnv: Record<string, string>) {
  const { url, child } = await startKernel(root, { KONAE_COPY_INTERVAL_SECONDS: "1", ...copyEnv });
  try {
    const owner = withHeaders(`${url}/mcp`, { cookie: await sign(url, "/_konae/owner/signup", { email: "owner@copy.test", password, setupKey }) });
    await owner.call("project_create", { projectId: "copy_proof", name: "Copy proof" });
    const payload = { project: { id: "copy_proof", name: "Copy proof", slug: "copy-proof", manifest: { version: 1, visibility: "public" }, files: project }, visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Copied app</main>" } } };
    const transfer = await owner.upload(new TextEncoder().encode(JSON.stringify(payload)), "copy_proof");
    const plan = await owner.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    const { id, hostname } = await owner.call<{ id: string; hostname: string }>("release_apply", { planId: plan.planId });
    await owner.cancel(transfer.transferId);

    const cookie = await sign(url, "/_konae/auth/signup", { email: "writer@copy.test", password }, hostname);
    const writer = withHeaders(`${url}/_konae/mcp`, { host: hostname, cookie });
    await writer.call("actions_invoke", { name: "notes.create", input: { title: "Before the copy" } });
    const upload = await writer.upload(attachment);
    const file = await writer.call<{ id: string }>("files_upload", { transferId: upload.transferId, name: "note.txt", type: "text/plain" });
    await writer.cancel(upload.transferId);

    const written = Date.now();
    let copies: { targets: { lastAttemptAt: number | null; lastSuccessAt: number | null; lastError: string | null }[]; alert: string | null } | undefined;
    for (let attempt = 0; attempt < 200; attempt++) {
      copies = (await (await fetch(`${url}/_konae/health`)).json()).copies;
      const done = copies!.targets.every((target) => target.lastAttemptAt !== null && target.lastAttemptAt > written && target.lastSuccessAt !== null && target.lastSuccessAt >= target.lastAttemptAt);
      if (done) break;
      await Bun.sleep(100);
    }
    expect(copies!.alert).toBeNull();
    expect(copies!.targets.every((target) => target.lastAttemptAt! > written && target.lastSuccessAt! >= target.lastAttemptAt!)).toBe(true);
    return { id, hostname, cookie, fileId: file.id };
  } finally {
    await stop(child);
  }
}

async function expectSameApp(root: string, app: { hostname: string; cookie: string; fileId: string }) {
  const { url, child } = await startKernel(root);
  try {
    const shell = await fetch(`${url}/`, { headers: { host: app.hostname } });
    expect(shell.status).toBe(200);
    expect(await shell.text()).toContain("<main>Copied app</main>");
    const stale = await withHeaders(`${url}/_konae/mcp`, { host: app.hostname, cookie: app.cookie }).call<{ role: string }>("auth_session");
    expect(stale.role).toBe("anonymous");
    const cookie = await sign(url, "/_konae/auth/login", { email: "writer@copy.test", password }, app.hostname);
    const writer = withHeaders(`${url}/_konae/mcp`, { host: app.hostname, cookie });
    expect(await writer.call("actions_invoke", { name: "notes.list" })).toEqual([{ id: 1, title: "Before the copy" }]);
    expect(await writer.download(await writer.call<Transfer>("files_read", { fileId: app.fileId }))).toEqual(attachment);
    const owner = withHeaders(`${url}/mcp`, { cookie: await sign(url, "/_konae/owner/login", { email: "owner@copy.test", password }) });
    // The project registry in mcp.sqlite travelled with the owner account.
    const inspection = await owner.call<{ deployment: { hostname: string } }>("project_inspect", { projectId: "copy_proof" });
    expect(inspection.deployment.hostname).toBe(app.hostname);
  } finally {
    await stop(child);
  }
}

describe("copies off the volume", () => {
  test("a directory copy restores into an empty data directory and serves the same app", async () => {
    const scratch = await mkdtemp(path.join(tmpdir(), "konae-copy-dir-"));
    try {
      const copies = path.join(scratch, "second-disk");
      const app = await publishAndCopy(path.join(scratch, "source"), { KONAE_COPY_DIR: copies });
      expect((await readdir(copies)).filter((name) => /^\d{4}-\d\d-\d\dT/.test(name)).length).toBeGreaterThan(0);
      const restored = path.join(scratch, "restored");
      const first = await restore(copies, restored);
      expect(first.output).toContain("checksums verified");
      expect(first.status).toBe(0);
      const again = await restore(copies, restored);
      expect(again.status).not.toBe(0);
      expect(again.output).toContain("not empty");
      await expectSameApp(restored, app);
    } finally {
      await rm(scratch, { recursive: true, force: true });
    }
  }, 120_000);

  // Needs the bucket to exist, as `docker compose up minio minio-init` creates it.
  // Copies go under copies/ and file bytes under files/ in that one bucket,
  // which other Kernels may share, so the test restores the copy that holds
  // its own Deployment by ID and removes only that copy and its files.
  test.skipIf(!minio)("an S3 copy restores into an empty data directory and serves the same app", async () => {
    const scratch = await mkdtemp(path.join(tmpdir(), "konae-copy-s3-"));
    const bucket = new Bun.S3Client({ endpoint: s3Endpoint, bucket: s3Bucket, accessKeyId: s3Env.KONAE_S3_ACCESS_KEY, secretAccessKey: s3Env.KONAE_S3_SECRET_KEY });
    const keys = async (prefix: string) => {
      const found: string[] = [];
      let continuationToken: string | undefined;
      do {
        const page = await bucket.list({ prefix, continuationToken });
        found.push(...(page.contents ?? []).map((object) => object.key));
        continuationToken = page.isTruncated ? page.nextContinuationToken : undefined;
      } while (continuationToken);
      return found;
    };
    /** IDs of the copies whose manifest names the Deployment's folder. */
    const copiesOf = async (deployment: string) => {
      const ids: string[] = [];
      for (const key of (await keys("copies/")).filter((key) => /^copies\/[^/]+\/manifest\.json$/.test(key))) {
        const manifest = await bucket.file(key).json().catch(() => undefined) as { files?: { path: string }[] } | undefined;
        if (manifest?.files?.some((file) => file.path.startsWith(`apps/${deployment}/`))) ids.push(key.split("/")[1]);
      }
      return ids.sort();
    };
    let deployment: string | undefined;
    try {
      const app = await publishAndCopy(path.join(scratch, "source"), { ...s3Env, KONAE_COPY_S3: "1" });
      deployment = app.id;
      const copy = (await copiesOf(deployment)).at(-1);
      expect(copy).toBeDefined();
      expect((await keys(`files/${deployment}/`)).length).toBeGreaterThan(0);
      const restored = path.join(scratch, "restored");
      const first = await restore(`s3://${s3Bucket}/copies/`, restored, s3Env, copy);
      expect(first.output).toContain("checksums verified");
      expect(first.status).toBe(0);
      await expectSameApp(restored, app);
    } finally {
      await rm(scratch, { recursive: true, force: true });
      if (deployment) {
        const copies = await copiesOf(deployment).catch(() => [] as string[]);
        const added = [...(await Promise.all(copies.map((copy) => keys(`copies/${copy}/`)))).flat(), ...await keys(`files/${deployment}/`).catch(() => [] as string[])];
        await Promise.all(added.map((key) => bucket.delete(key)));
      }
    }
  }, 120_000);
});
