import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";

let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let owner: McpClient;
let reader: McpClient;
let publisher: McpClient;
const projectId = "security_mcp_primary";
const otherProjectId = "security_mcp_other";
const port = Number(process.env.KONAE_SECURITY_MCP_PORT ?? 19600);
const origin = `http://127.0.0.1:${port}`;
const setupKey = crypto.randomUUID();

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "konae-security-mcp-"));
  kernel = Bun.spawn(["kernel-rs/target/debug/konae"], {
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))),
      KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_PUBLIC_ORIGIN: origin,
      KONAE_HOST_SUFFIX: "localhost", KONAE_DATA_DIR: root, KONAE_SETUP_KEY: setupKey,
    }, stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (kernel.exitCode !== null) throw Error("Security test Kernel stopped.");
    try { ready = (await fetch(`${origin}/_konae/health`)).ok; } catch {}
    if (ready) break;
    await Bun.sleep(100);
  }
  expect(ready).toBe(true);
  const response = await fetch(`${origin}/_konae/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@security.test", password: setupKey, setupKey }),
  });
  expect(response.status).toBe(200);
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  owner = new McpClient(`${origin}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  for (const id of [projectId, otherProjectId]) await owner.call("project_create", { projectId: id, name: id });
  const delegate = async (scopes: string[]) => {
    const { token } = await owner.call<{ token: string }>("token_create", { scopes, projectIds: [projectId], ttlSeconds: 60 });
    return new McpClient(`${origin}/mcp`, { token });
  };
  reader = await delegate(["read"]);
  publisher = await delegate(["publish"]);
}, 120_000);

afterAll(async () => {
  kernel?.kill();
  if (kernel) await kernel.exited;
  if (root) await rm(root, { recursive: true, force: true });
});

const database = () => new Database(path.join(root, "mcp.sqlite"));

describe("MCP security controls", () => {
  test("denied scoped read requests keep bounded Unicode-safe audit targets", async () => {
    const target = "🐱".repeat(30_000);
    for (let attempt = 0; attempt < 30; attempt++) {
      await expect(reader.call("project_inspect", { projectId: target })).rejects.toThrow("outside this token's grants");
    }
    const db = database();
    try {
      const row = db.query("SELECT COUNT(*) count,MAX(length(target)) maxCharacters,SUM(length(CAST(target AS BLOB))) bytes FROM audit WHERE tool='project_inspect'").get() as { count: number; maxCharacters: number; bytes: number };
      expect(row.count).toBe(30);
      expect(row.maxCharacters).toBe(128);
      expect(row.bytes).toBe(30 * 128 * 4);
      const latest = db.query("SELECT actor,target,ok FROM audit WHERE tool='project_inspect' ORDER BY id DESC LIMIT 1").get() as { actor: string; target: string; ok: number };
      expect(latest.actor).toStartWith("owner:");
      expect(latest.target).toBe("🐱".repeat(128));
      expect(latest.ok).toBe(0);
    } finally { db.close(); }
  });

  test("actual tool calls prune audit row count and expired history in batches", async () => {
    const db = database();
    try {
      db.exec("DELETE FROM audit");
      const insert = db.prepare("INSERT INTO audit(at,actor,tool,target,ok) VALUES(?1,'owner:seed','seed','seed',0)");
      db.transaction(() => {
        for (let index = 0; index < 10_050; index++) insert.run(index < 9_900 ? Date.now() : 0);
      })();
    } finally { db.close(); }
    for (let attempt = 0; attempt < 128; attempt++) {
      await expect(reader.call("project_inspect", { projectId: "outside_grant" })).rejects.toThrow("outside this token's grants");
    }
    const inspect = database();
    try {
      const row = inspect.query("SELECT COUNT(*) count,SUM(at=0) expired,SUM(tool='project_inspect') denied FROM audit").get() as { count: number; expired: number; denied: number };
      expect(row.count).toBeLessThanOrEqual(10_127);
      expect(row.expired).toBe(0);
      expect(row.denied).toBe(128);
    } finally { inspect.close(); }
  });

  test("publish-only credentials can cancel their upload without reading it or another project", async () => {
    const upload = { projectId, size: 1, sha256: "0".repeat(64) };
    const transfer = await publisher.call<{ transferId: string }>("transfer_begin", upload);
    await expect(publisher.call("transfer_read", { transferId: transfer.transferId, offset: 0 })).rejects.toThrow("Permission denied for read");
    expect(await publisher.cancel(transfer.transferId)).toMatchObject({ ok: true });
    const other = await owner.call<{ transferId: string }>("transfer_begin", { ...upload, projectId: otherProjectId });
    try {
      await expect(publisher.cancel(other.transferId)).rejects.toThrow("outside this token's grants");
      expect(await owner.call("transfer_status", { transferId: other.transferId })).toMatchObject({ transferId: other.transferId });
    } finally { await owner.cancel(other.transferId); }
  });
});
