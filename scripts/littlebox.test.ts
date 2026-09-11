import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { ignoreInGit, publishLocal } from "./littlebox";

/** A stand-in Kernel MCP endpoint that records tool calls and can fail chosen ones. */
const calls: Array<{ name: string; arguments: Record<string, unknown> }> = [];
const failing = new Set<string>();
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
        release_apply: { url: "http://test.localhost/" },
      };
      return reply({ isError: false, structuredContent: { result: results[name] ?? {} } });
    },
  });
});

afterAll(() => server.stop(true));

let directory: string;
beforeEach(async () => {
  calls.length = 0;
  failing.clear();
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

  test("ignoreInGit adds an entry once", async () => {
    const file = path.join(directory, ".gitignore");
    await writeFile(file, "node_modules\n.littlebox/");
    await ignoreInGit(directory, [".littlebox/", "node_modules/", "dist/"]);
    await ignoreInGit(directory, [".littlebox/", "dist/"]);
    expect(await readFile(file, "utf8")).toBe("node_modules\n.littlebox/\ndist/\n");
  });
});
