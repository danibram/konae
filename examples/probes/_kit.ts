// The harness every probe shares. It compiles the Kernel offline, or takes the
// konae binary KONAE_BIN names, starts it on
// KONAE_PROBE_PORT with a random setup key, the development mailbox and a
// data directory under the system temp folder, signs up the owner, and removes
// the Kernel and its data when the probe ends, whether it passed or not.
//
// A probe is small. It publishes one Capsule from its own folder and asserts
// the answers that prove one kind of app works, or the refusal both doors give.
import assert from "node:assert/strict";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import initSqlJs from "sql.js";
import { McpClient } from "../../src/mcp/client";
import { appDir, buildLocal } from "../../scripts/konae";
import { createCapsuleValidator } from "../../src/agent/gate";

const repo = path.resolve(import.meta.dir, "../..");

export type Receipt = { id: string; hostname: string; url: string };
export type Published = { receipt: Receipt; host: string; deploymentId: string };

export type Kit = {
  url: string;
  dataDir: string;
  owner: McpClient;
  pass(flow: string, result: string): void;
  /** Publishes the Capsule in `dir` as a first release and returns its host. */
  publish(dir: string, options: { projectId: string; visibility: "public" | "private"; signup?: { mode: string; allowlist?: string[] }; files?: Record<string, string>; secrets?: Record<string, string>; initialPackage?: unknown; manifest?: Record<string, unknown> }): Promise<Published>;
  /** The message release_plan answers for a Capsule whose sources are overridden by `files`. */
  planRefusal(dir: string, projectId: string, files: Record<string, string>): Promise<string>;
  /** The editor's Validation Gate on the same sources: its message and, when it has one, the detail under it. */
  gateRefusal(files: { "schema.sql": string; "actions.json": string }): Promise<{ message: string; detail?: string }>;
  /** fetch on the app's host, from one client address, with an optional app cookie. */
  appFetch(host: string, options?: { cookie?: string; address?: string }): typeof fetch;
  /** The app's runtime MCP endpoint, as the SDK calls it. */
  runtime(host: string, cookie?: string): McpClient;
  /** Signs a person up under Open signup in local verification mode and returns the session cookie. */
  signup(host: string, email: string): Promise<string>;
  /** A fresh client address, so the anonymous rate limit does not leak between steps. */
  address(): string;
  errorText(promise: Promise<unknown>): Promise<string>;
};

/** `env` adds to the Kernel's environment, such as KONAE_DNS_RESOLVER for a fake resolver. */
export async function probe(name: string, defaultPort: number, body: (kit: Kit) => Promise<void>, options: { env?: Record<string, string> } = {}) {
  const port = Number(process.env.KONAE_PROBE_PORT ?? defaultPort);
  assert(Number.isInteger(port) && port >= 1024 && port <= 65535, "Set KONAE_PROBE_PORT to a free local port.");
  const url = `http://127.0.0.1:${port}`;
  const scratch = await mkdtemp(path.join(tmpdir(), `konae-probe-${name}-`));
  const dataDir = path.join(scratch, "data");
  const setupKey = crypto.randomUUID();
  let kernel: ReturnType<typeof Bun.spawn> | undefined;
  let counter = 0;
  const address = () => { counter += 1; return `198.18.${Math.floor(counter / 250)}.${(counter % 250) + 1}`; };
  const errorText = async (promise: Promise<unknown>) => {
    try { await promise; } catch (error) { return error instanceof Error ? error.message : String(error); }
    throw Error("Expected a refusal, but the call succeeded.");
  };
  try {
    try { await fetch(`${url}/_konae/health`); throw Error(`Something already answers on port ${port}. Set KONAE_PROBE_PORT.`); }
    catch (error) { if (error instanceof Error && error.message.startsWith("Something")) throw error; }
    // KONAE_BIN runs a konae binary already built from these sources instead of compiling one.
    const binary = process.env.KONAE_BIN ?? path.join(repo, "kernel-rs/target/debug/konae");
    if (!process.env.KONAE_BIN) {
      const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { cwd: repo, stdout: "ignore", stderr: "inherit" });
      assert.equal(await build.exited, 0, "The Kernel must compile.");
    }
    kernel = Bun.spawn([binary], {
      cwd: repo,
      env: {
        ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))),
        KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_PUBLIC_ORIGIN: url,
        KONAE_HOST_SUFFIX: "localhost", KONAE_DATA_DIR: dataDir, KONAE_ADMIN_KEY: setupKey,
        KONAE_TRUSTED_PROXIES: "127.0.0.1", KONAE_MAIL_TRANSPORT: "development",
        // The schedule probe sets it to watch a run in seconds instead of minutes.
        ...(process.env.KONAE_SCHEDULE_MIN_SECONDS ? { KONAE_SCHEDULE_MIN_SECONDS: process.env.KONAE_SCHEDULE_MIN_SECONDS } : {}),
        ...options.env,
      },
      stdout: "ignore", stderr: "inherit",
    });
    let ready = false;
    for (let attempt = 0; attempt < 100 && !ready; attempt++) {
      assert.equal(kernel.exitCode, null, "The Kernel stopped before startup. Is the port taken?");
      try { ready = (await fetch(`${url}/_konae/health`)).ok; } catch {}
      if (!ready) await Bun.sleep(100);
    }
    assert(ready, "The Kernel did not start.");

    const signup = await fetch(`${url}/_konae/owner/signup`, {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ email: "owner@probe.example", password: crypto.randomUUID(), setupKey }),
    });
    assert.equal(signup.status, 200, await signup.clone().text());
    const ownerCookie = signup.headers.get("set-cookie")!.split(";")[0];
    const owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
      const headers = new Headers(init?.headers); headers.set("cookie", ownerCookie);
      return fetch(input, { ...init, headers });
    } });

    // The probe's sources go into app/ of a scratch app folder, the layout buildLocal reads.
    const capsule = async (dir: string, files: Record<string, string> = {}) => {
      const copy = await mkdtemp(path.join(scratch, "capsule-"));
      const app = appDir(copy);
      await cp(dir, app, { recursive: true, filter: (source) => !source.endsWith("replay.ts") && !source.endsWith("README.md") });
      for (const [file, text] of Object.entries(files)) await writeFile(path.join(app, file), text);
      const { artifact } = await buildLocal(copy);
      const sources: Record<string, string> = {};
      for (const file of ["index.html", "schema.sql", "actions.json"]) sources[file] = await readFile(path.join(app, file), "utf8");
      return { artifact, sources };
    };
    const created = new Set<string>();
    const release = async (dir: string, projectId: string, visibility: "public" | "private", files: Record<string, string>, extra: object = {}, secrets: Record<string, string> = {}, manifest: Record<string, unknown> = {}) => {
      const project = { id: projectId, name: projectId, slug: projectId.replaceAll("_", "-"), manifest: { version: 1, visibility, ...manifest } };
      if (!created.has(projectId)) {
        await owner.call("project_create", { projectId, name: project.name, slug: project.slug, manifest: project.manifest });
        created.add(projectId);
      }
      for (const [name, value] of Object.entries(secrets)) await owner.call("secret_set", { projectId, name, value });
      const { artifact, sources } = await capsule(dir, files);
      const bytes = new TextEncoder().encode(JSON.stringify({ project: { ...project, files: sources }, visibility, artifact, ...extra }));
      const upload = await owner.upload(bytes, projectId);
      try {
        const plan = await owner.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
        return await owner.call<Receipt>("release_apply", { planId: plan.planId });
      } finally { await owner.cancel(upload.transferId).catch(() => {}); }
    };
    const gate = createCapsuleValidator(() => initSqlJs());

    const appFetch = (host: string, options: { cookie?: string; address?: string } = {}): typeof fetch => (input, init) => {
      const headers = new Headers(init?.headers);
      headers.set("host", host);
      headers.set("x-forwarded-for", options.address ?? address());
      if (options.cookie) headers.set("cookie", options.cookie);
      return fetch(input, { ...init, headers });
    };
    const kit: Kit = {
      url, dataDir, owner, address, errorText, appFetch,
      pass: (flow, result) => console.log(`PASS ${flow}. ${result}`),
      async publish(dir, options) {
        const receipt = await release(dir, options.projectId, options.visibility, options.files ?? {}, {
          ...(options.signup ? { initialSignupPolicy: { mode: options.signup.mode, allowlist: options.signup.allowlist ?? [] } } : {}),
          ...(options.initialPackage ? { initialPackage: options.initialPackage } : {}),
        }, options.secrets, options.manifest);
        return { receipt, host: receipt.hostname, deploymentId: receipt.id };
      },
      planRefusal: (dir, projectId, files) => errorText(release(dir, projectId, "public", files)),
      async gateRefusal(files) {
        const result = await gate.validate(files as never);
        assert.equal(result.ok, false, "The Validation Gate passed sources it should refuse.");
        return { message: result.errors[0].message, detail: result.errors[0].detail };
      },
      runtime: (host, cookie) => new McpClient(`${url}/_konae/mcp`, { fetch: appFetch(host, { cookie }) }),
      async signup(host, email) {
        const answer = await appFetch(host)(`${url}/_konae/auth/signup`, {
          method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, password: crypto.randomUUID() }),
        });
        assert.equal(answer.status, 200, await answer.clone().text());
        return answer.headers.get("set-cookie")!.split(";")[0];
      },
    };
    await body(kit);
    console.log(`Probe ${name} passed.`);
  } catch (error) {
    console.error(error);
    process.exitCode = 1;
  } finally {
    kernel?.kill();
    await kernel?.exited;
    await rm(scratch, { recursive: true, force: true });
  }
}
