import { setDefaultTimeout } from "bun:test";
import { cp, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildLocal } from "../scripts/konae";
import { McpClient } from "../src/mcp/client";
import { buildKernel, freshClient, password, release, startKernel, type Kernel } from "./kernel-harness";

export { freshClient, password };

setDefaultTimeout(30_000);
export const repo = path.resolve(import.meta.dir, "..");
const showcase = path.join(repo, "examples/showcase");

export type App = { id: string; hostname: string; url: string; projectId: string };
export type Lab = {
  kernel: Kernel;
  root: string;
  apps: Record<"inbox" | "club" | "gallery" | "habits" | "booking", App>;
  secrets: Record<string, string>;
  stop(): Promise<void>;
};

const SHOWCASE = {
  inbox: { visibility: "private", secrets: ["events_webhook", "events_feed", "github_webhook", "payments_webhook"] },
  club: { visibility: "private", secrets: [] },
  gallery: { visibility: "public", secrets: [] },
  habits: { visibility: "private", secrets: [] },
  booking: { visibility: "public", secrets: ["panel_hook"] },
} as const;

export function testPort(variable: string, base: number) {
  const configured = Number(process.env[variable]);
  return Number.isInteger(configured) && configured > 1024 ? configured : base + Math.floor(Math.random() * 200);
}

export async function startLab(port: number, env: Record<string, string> = {}): Promise<Lab> {
  await buildKernel();
  const root = await mkdtemp(path.join(tmpdir(), "konae-security-review-"));
  const kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: crypto.randomUUID(), ownerEmail: "owner@review.test", env });
  const apps = {} as Lab["apps"];
  const secrets: Record<string, string> = {};
  try {
    for (const [name, spec] of Object.entries(SHOWCASE) as [keyof typeof SHOWCASE, (typeof SHOWCASE)[keyof typeof SHOWCASE]][]) {
      const source = path.join(root, name);
      await cp(path.join(showcase, name), path.join(source, "app"), { recursive: true, filter: (from) => !from.endsWith("replay.ts") && !from.endsWith("README.md") });
      const compiled = await buildLocal(source);
      const project = { id: `review_${name}`, name, slug: name, manifest: { version: 1, visibility: spec.visibility } };
      await kernel.owner.call("project_create", { projectId: project.id, name, slug: name, manifest: project.manifest });
      for (const secret of spec.secrets) {
        secrets[secret] = `${secret}_${crypto.randomUUID().replaceAll("-", "")}`;
        await kernel.owner.call("secret_set", { projectId: project.id, name: secret, value: secrets[secret] });
      }
      const { receipt } = await release(kernel.owner, {
        project: { ...project, files: compiled.files }, visibility: spec.visibility, artifact: compiled.artifact,
        initialSignupPolicy: { mode: "open", allowlist: [] },
      }, project.id);
      apps[name] = receipt;
    }
  } catch (error) {
    await kernel.stop();
    await rm(root, { recursive: true, force: true });
    throw error;
  }
  return {
    kernel, root, apps, secrets,
    async stop() { await kernel.stop(); await rm(root, { recursive: true, force: true }); },
  };
}

export function hostFetch(lab: Lab, hostname: string, route: string, init: RequestInit & { client?: string } = {}) {
  const headers = new Headers(init.headers);
  headers.set("host", hostname);
  if (!headers.has("x-forwarded-for")) headers.set("x-forwarded-for", init.client ?? freshClient());
  if (init.body && !headers.has("content-type")) headers.set("content-type", "application/json");
  return fetch(`${lab.kernel.url}${route}`, { ...init, headers, redirect: "manual" });
}

export async function appUser(lab: Lab, app: App, email = `${crypto.randomUUID().slice(0, 8)}@review.test`) {
  const response = await hostFetch(lab, app.hostname, "/_konae/auth/signup", { method: "POST", body: JSON.stringify({ email, password }) });
  if (response.status !== 200) throw Error(`Signup for ${email} answered ${response.status}: ${await response.text()}`);
  return { email, cookie: response.headers.get("set-cookie")!.split(";")[0], token: response.headers.get("set-cookie")!.split(";")[0].split("=")[1] };
}

export function runtimeClient(lab: Lab, hostname: string, cookie?: string, client = freshClient()) {
  return new McpClient(`${lab.kernel.url}/_konae/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("host", hostname);
    headers.set("x-forwarded-for", client);
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

export async function rawCall(lab: Lab, route: "/mcp" | "/_konae/mcp", hostname: string, name: string, args: object, headers: Record<string, string> = {}) {
  const response = await hostFetch(lab, hostname, route, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json, text/event-stream", ...headers },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } }),
  });
  const text = await response.text();
  let body: any = text;
  try { body = JSON.parse(text); } catch {}
  return { status: response.status, body, text };
}

export function answerText(body: any): string {
  if (body?.error?.message) return body.error.message;
  if (typeof body?.error === "string") return body.error;
  return body?.result?.content?.map((part: { text?: string }) => part.text ?? "").join("") ?? JSON.stringify(body);
}
