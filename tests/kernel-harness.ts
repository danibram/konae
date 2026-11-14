// Starts Kernels, publishes apps on them and imports a package into a new
// project. Used by the security review and check-carry suites.
import { open, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { McpClient, encodeBase64, type Transfer } from "../src/mcp/client";
import type { PackageManifest } from "../src/lib/portable-package";

const repo = path.resolve(import.meta.dir, "..");
export const kernelBinary = path.join(repo, "kernel-rs/target/debug/konae");
export const password = "kernel-harness-password";
let nextClient = 0;
/** A client address per request, so the signup and sign-in limits never trip. */
export const freshClient = () => { nextClient++; return `198.18.${Math.floor(nextClient / 250) % 250}.${nextClient % 250 + 1}`; };

export async function buildKernel() {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", path.join(repo, "kernel-rs/Cargo.toml")], { stdout: "ignore", stderr: "inherit" });
  if (await build.exited !== 0) throw Error("cargo build failed.");
}

export type Kernel = {
  url: string;
  data: string;
  env: Record<string, string>;
  owner: McpClient;
  ownerCookie: string;
  stop(): Promise<void>;
  start(): Promise<void>;
  mailbox(): Promise<{ to: string; purpose: string; link: string; scope: string }[]>;
};

/** Starts a Kernel on `port` with its data in `data`, and signs up its owner, or signs in an existing one. */
export async function startKernel(options: { port: number; data: string; setupKey: string; ownerEmail: string; env?: Record<string, string>; signIn?: boolean }): Promise<Kernel> {
  const url = `http://127.0.0.1:${options.port}`;
  const env = {
    ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))) as Record<string, string>,
    KONAE_DATA_DIR: options.data, KONAE_KERNEL_PORT: String(options.port), KONAE_BIND: "127.0.0.1",
    KONAE_SETUP_KEY: options.setupKey, KONAE_PUBLIC_ORIGIN: url, KONAE_HOST_SUFFIX: "localhost",
    KONAE_TRUSTED_PROXIES: "127.0.0.1", KONAE_MAIL_TRANSPORT: "development",
    ...options.env,
  };
  let child: ReturnType<typeof Bun.spawn> | undefined;
  const start = async () => {
    child = Bun.spawn([kernelBinary], { env, stdout: "ignore", stderr: "inherit" });
    for (let attempt = 0; attempt < 150; attempt++) {
      try { if ((await fetch(`${url}/_konae/health`)).ok) return; } catch {}
      if (child.exitCode !== null) throw Error(`The Kernel on ${url} stopped during startup.`);
      await Bun.sleep(100);
    }
    throw Error(`The Kernel on ${url} did not start.`);
  };
  const stop = async () => { if (child && child.exitCode === null) { child.kill(); await child.exited; } };
  await start();
  const response = await fetch(`${url}/_konae/owner/${options.signIn ? "login" : "signup"}`, {
    method: "POST", headers: { "content-type": "application/json", "x-forwarded-for": freshClient() },
    body: JSON.stringify(options.signIn ? { email: options.ownerEmail, password } : { email: options.ownerEmail, password, setupKey: options.setupKey }),
  });
  if (response.status !== 200) throw Error(`Owner ${options.signIn ? "sign-in" : "signup"} on ${url} failed: ${response.status} ${await response.text()}`);
  const ownerCookie = response.headers.get("set-cookie")!.split(";")[0];
  const owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", ownerCookie);
    return fetch(input, { ...init, headers });
  } });
  const mailbox = async () => (await Bun.file(path.join(options.data, "mailbox.jsonl")).text().catch(() => ""))
    .trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
  return { url, data: options.data, env, owner, ownerCookie, stop, start, mailbox };
}

export function runtime(kernel: Kernel, hostname: string, cookie?: string) {
  return new McpClient(`${kernel.url}/_konae/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", hostname);
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

/** A request to an app host with its own client address. */
export function appFetch(kernel: Kernel, hostname: string, route: string, init: RequestInit = {}) {
  const headers = new Headers(init.headers);
  headers.set("host", hostname);
  headers.set("x-forwarded-for", freshClient());
  if (init.body && !headers.has("content-type")) headers.set("content-type", "application/json");
  return fetch(`${kernel.url}${route}`, { ...init, headers, redirect: "manual" });
}

/** Signs up or in on an app and returns the session cookie. */
export async function account(kernel: Kernel, hostname: string, route: "signup" | "login", email: string) {
  const response = await appFetch(kernel, hostname, `/_konae/auth/${route}`, { method: "POST", body: JSON.stringify({ email, password }) });
  if (response.status !== 200) throw Error(`${route} for ${email} answered ${response.status}: ${await response.text()}`);
  return response.headers.get("set-cookie")!.split(";")[0];
}

export async function release(client: McpClient, payload: object, projectId: string) {
  const upload = await client.upload(new TextEncoder().encode(JSON.stringify(payload)), projectId);
  try {
    const plan = await client.call<{ planId: string; applied?: boolean; receipt?: Deployment }>("release_plan", { transferId: upload.transferId });
    const receipt = await client.call<Deployment>("release_apply", { planId: plan.planId });
    return { plan, receipt };
  } finally { await client.cancel(upload.transferId).catch(() => {}); }
}

export type Deployment = { id: string; hostname: string; url: string; projectId: string };

/** A downloaded package: each component's file, and the release payload under `release`. */
export type Captured = { manifest: PackageManifest; files: Record<string, string> };

async function uploadFile(client: McpClient, file: string, content: { name: string; size: number; sha256: string }, projectId: string) {
  const transfer = await client.call<Transfer>("transfer_begin", { projectId, size: content.size, sha256: content.sha256, component: content.name });
  const handle = await open(file, "r");
  try {
    const buffer = new Uint8Array(transfer.chunkBytes);
    for (let offset = 0; offset < content.size;) {
      const { bytesRead } = await handle.read(buffer, 0, Math.min(buffer.length, content.size - offset), offset);
      await client.call("transfer_write", { transferId: transfer.transferId, offset, data: encodeBase64(buffer.subarray(0, bytesRead)) });
      offset += bytesRead;
    }
  } finally { await handle.close(); }
  await client.call("transfer_complete", { transferId: transfer.transferId });
  return transfer.transferId;
}

/** Imports a downloaded package into `projectId`, creating the project if needed. */
export async function importPackage(client: McpClient, captured: Captured, projectId: string, name = "Imported app") {
  await client.call("project_create", { projectId, name }).catch((error: Error) => {
    if (!/exists|already/i.test(error.message)) throw error;
  });
  const components: Record<string, string> = {};
  for (const content of captured.manifest.contents) components[content.name] = await uploadFile(client, captured.files[content.name], content, projectId);
  const payload = JSON.parse(await readFile(captured.files.release, "utf8"));
  payload.project.id = projectId;
  payload.initialPackage = { manifest: captured.manifest, components };
  try {
    return await release(client, payload, projectId);
  } finally {
    for (const id of Object.values(components)) await client.cancel(id).catch(() => {});
  }
}

/** Writes `bytes` to a file and returns its path. */
export async function scratchFile(directory: string, name: string, bytes: string | Uint8Array) {
  const file = path.join(directory, name);
  await writeFile(file, bytes);
  return file;
}
