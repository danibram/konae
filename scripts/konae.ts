#!/usr/bin/env bun
import { chmod, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { watch } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { McpClient, type Transfer } from "../src/mcp/client";
import { normalizeManifest } from "../src/deployments/manifest";
import { packageComponentFiles, type PackageManifest } from "../src/lib/portable-package";
import { writeTarFile, type FileTarEntry } from "./package-files";
import { PROJECT_TEMPLATES_FILE, type ProjectTemplates } from "./project-templates";
import type { AppEvent } from "./events";
import { PULL_DIRECTORY, describePulled, downloadPackage, installPulledAlone, pullLocal, readPulled, uploadPulled, type Pulled } from "./pull";
import {
  ACTION_CONTRACT_FILE, ACTION_CONTRACT_MARKDOWN, ACTION_CONTRACT_SCHEMA_FILE,
  actionContractSchema, generateActionContract, renderActionContractMarkdown, serializeActionContract,
} from "../src/lib/action-contract";
import { checkHostname, type DomainRules } from "../src/lib/domain-name";
import { assembleStaticArtifact } from "../src/lib/static-artifact.js";

const toolRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
type Project = { id: string; name: string; slug: string; manifest: Record<string, unknown>; files?: Record<string, string> };
/** `url` is absent in a project that `init` wrote without one, until a publish names the installation. */
type Config = { url?: string; project: Project };
type SignupMode = "open" | "invitation" | "closed" | "allowlist";
export type SignupPolicy = { mode: SignupMode; allowlist: string[] };

const SIGNUP_MODES: readonly SignupMode[] = ["open", "invitation", "closed", "allowlist"];
const MODE_LABELS: Record<SignupMode, string> = { open: "Open", invitation: "Invitation only", closed: "Closed", allowlist: "Allowlist" };
/** What `init` writes: a private app that admits the people its owner invites. */
export const INIT_SIGNUP_POLICY = { mode: "invitation" } as const;

/**
 * `project.manifest.signupPolicy` in konae.json: who may create an app
 * account when the first release goes out. Checked before anything is sent;
 * the Kernel checks the allowlist entries again.
 */
export function manifestSignupPolicy(manifest: Record<string, unknown>): SignupPolicy | undefined {
  const value = manifest.signupPolicy as { mode?: unknown; allowlist?: unknown } | undefined;
  if (value === undefined) return undefined;
  if (!value || typeof value !== "object" || !SIGNUP_MODES.includes(value.mode as SignupMode)) {
    throw Error('konae.json project.manifest.signupPolicy.mode must be "open", "invitation", "closed" or "allowlist".');
  }
  const allowlist = value.allowlist ?? [];
  if (!Array.isArray(allowlist) || !allowlist.every((entry) => typeof entry === "string")) {
    throw Error("konae.json project.manifest.signupPolicy.allowlist must be a list of emails and @domains.");
  }
  return { mode: value.mode as SignupMode, allowlist };
}

function describePolicy(policy: SignupPolicy) {
  return policy.mode === "allowlist" ? `Allowlist (${policy.allowlist.join(", ") || "empty"})` : MODE_LABELS[policy.mode];
}

/**
 * The warning a republish prints when konae.json names another policy
 * than the Deployment has. A publish never changes who may join an app that
 * exists (docs/mcp.md, "Signup policy"), so the file and the app can differ.
 */
export function signupPolicyWarning(wanted: SignupPolicy, current: SignupPolicy): string | undefined {
  const entries = (policy: SignupPolicy) => policy.allowlist.map((entry) => entry.trim().toLowerCase()).filter(Boolean).sort().join("\n");
  if (wanted.mode === current.mode && (wanted.mode !== "allowlist" || entries(wanted) === entries(current))) return undefined;
  return `konae.json asks for signup ${describePolicy(wanted)}, but this app has ${describePolicy(current)}. A publish sets the policy only on the first release and never changes who may join afterwards. Change it in the editor's Auth > Users view, or call signup_policy_set with a credential that holds manage_users.`;
}

/**
 * The scopes `login` asks for: read, write and publish always, and each
 * permission a flag names. A delegated token cannot widen itself later, so
 * the flags are the only way the CLI gets them.
 */
export function loginScopes(args: string[]) {
  return ["read", "write", "publish",
    ...(args.includes("--manage-users") ? ["manage_users"] : []),
    ...(args.includes("--export-data") ? ["export_data"] : [])];
}

/** The templates `init --template` takes: the ids of src/lib/templates, which the editor's new-project dialog and project_create offer too. */
export const INIT_TEMPLATES = ["tasks", "notes", "members", "form", "webhook", "digest", "gallery"] as const;

/** Where a running `dev` leaves its Kernel's address, owner token and Deployment for `sql` and `events`. */
const DEV_SESSION = ".konae/dev.json";

/** What publish, pull and export say when nothing names the installation. */
export const NO_INSTALLATION = "No installation to talk to. Pass --url <Kernel URL>, or run login in this folder, which saves the one it signed in to.";

/** What `konae help` prints: every command with its options, in the order a project meets them. */
export const HELP = `Usage: bun run konae -- <command> [options]

login     --dir <folder> --url <Kernel URL> [--manage-users] [--export-data]
          [--verify-from <mailbox.jsonl>]
          Reads KONAE_EMAIL and KONAE_PASSWORD, plus KONAE_INVITATION_CODE to create the
          account with an operator's invitation code, or KONAE_SETUP_KEY for the first owner.
          Saves a 24-hour token with read, write and publish in <folder>/.konae/credentials.json.
          Each flag adds that permission: inviting app users and exporting live data.
          On a Kernel that verifies email, creating the account sends a link; --verify-from reads it from
          a development mailbox on this machine and sets KONAE_PASSWORD, for loopback Kernels only.
init      --dir <empty folder> [--name <name>] [--template <id>] [--url <Kernel URL>]
          Writes a template and konae.json. It needs no credential and no Kernel, and the first
          publish registers the project. Templates: ${INIT_TEMPLATES.join(", ")}.
dev       --dir <folder> [--port 8796] [--events-port 8797] [--reset]
          Runs the app on a local Development Kernel and republishes on change.
          Its email, notices included, goes to <folder>/.konae/dev-data/mailbox.jsonl.
          It sets and prints a local value for each signing secret actions.json names, and serves a
          page of what the app did (email, calls, scheduled runs, Signals) on the next port.
          KONAE_KERNEL_BIN names a built Kernel binary to run instead of compiling one.
          --reset deletes <folder>/.konae/dev-data first. A Development Kernel with no data starts
          from the copy pull left, when there is one.
sql       --dir <folder> [--prod] [--url <Kernel URL>] ("<statement>" | --file <file.sql>)
          Runs SQL on the app's database through database_execute, the owner's door, on the
          Development Kernel that dev is running for this folder. Only --prod reaches the published
          Deployment.
events    --dir <folder> [--follow] [--json] [--days 7] [--prod]
          Lists what the app did besides answer its page: email, calls to other services, scheduled
          runs, routes and Signals, oldest first. On the Development Kernel an email shows its
          recipient and text, because nothing is sent. --json prints one object per line and --follow
          keeps printing. Only --prod reads the published Deployment.
pull      --dir <folder> [--url <Kernel URL>]
          Downloads the published app's database, accounts and files into <folder>/.konae/pull.
          Needs a credential from login --export-data. Run dev --reset to start the local app from it.
build     --dir <folder>                 Writes .konae/artifact.json.
contract  --dir <folder> [--out <folder>]   Writes action-contract.json, its schema and action-contract.md.
publish   --dir <folder> [--url <Kernel URL>]
          Builds and publishes to the installation konae.json names, or --url, or the one login
          saved. Set each signing secret with secret_set first.
conform   --url <app URL> --contract <file> [--token <app session>] [--key <API key>]
          [--secret <name>=<value>] [--inputs <file>]
          Calls every action and checks the answers against the contract. It runs each mutation once,
          so point it at a test Deployment.
export    --dir <folder> [--out <file>] [--with-data]   Writes a self-hosted kit; --with-data adds the
          live package and needs a credential from login --export-data.
domain    --deployment <id> [--set <hostname> | --clear] [--url <Kernel URL>]
          Shows, sets or clears the app's custom domain. --set prints the TXT record that proves you
          control the hostname; run it again once the record is at your DNS provider.

init, build, contract and dev need no credential. Every other command but login reads it from
<folder>/.konae/credentials.json or KONAE_MCP_TOKEN.`;

function endpoint(url: string) { return new URL("/mcp", url).href; }
async function json<T>(filename: string): Promise<T> { return JSON.parse(await readFile(filename, "utf8")); }

/** What login says when the account it created waits for its emailed link. */
export const VERIFY_BY_LINK = "This installation verifies email, so the account waits for the link sent to your address. Open it, choose KONAE_PASSWORD as the password, then run login again with the same KONAE_EMAIL and KONAE_PASSWORD and without the code or key, which are used now. On a development Kernel on this machine, run login with --verify-from <its data folder>/mailbox.jsonl to do both in one step.";

/** Whether a Kernel URL names this machine, the only place a development mailbox can be read. */
export function isLoopbackUrl(url: string) {
  const host = new URL(url).hostname.replace(/^\[|\]$/g, "");
  return host === "localhost" || host.endsWith(".localhost") || host === "::1" || /^127(\.\d{1,3}){3}$/.test(host);
}

/**
 * The newest creator verification token for `email` among the development
 * mailbox's lines past the first `seen`. Lines for app accounts, other
 * addresses and other purposes are skipped.
 */
export function creatorVerificationToken(mailbox: string, email: string, seen = 0): string | undefined {
  const lines = mailbox.split("\n").filter(Boolean).slice(seen);
  const message = lines.map((line) => { try { return JSON.parse(line) as { to?: string; scope?: string; purpose?: string; link?: string }; } catch { return {}; } })
    .filter((item) => item.scope === "creator" && item.purpose === "verification" && item.to?.toLowerCase() === email.toLowerCase() && item.link)
    .at(-1);
  return message ? new URLSearchParams(new URL(message.link!).hash.slice(1)).get("verification") ?? undefined : undefined;
}

async function mailboxLines(file: string) {
  return (await readFile(file, "utf8").catch(() => "")).split("\n").filter(Boolean).length;
}

/** Waits for the Kernel's mail worker to write the link, then answers it with the password. */
async function verifyFromMailbox(url: string, file: string, email: string, password: string, seen: number) {
  for (const deadline = Date.now() + 15_000; Date.now() < deadline; await Bun.sleep(200)) {
    const token = creatorVerificationToken(await readFile(file, "utf8").catch(() => ""), email, seen);
    if (!token) continue;
    return fetch(new URL("/_konae/owner/verify", url), {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ token, password }),
    });
  }
  throw Error(`No verification link for ${email} reached ${file} within 15 seconds. Check that this is the mailbox of the Kernel at ${url}, which writes one when KONAE_MAIL_TRANSPORT=development.`);
}

/** Signs in, or creates the account with an invitation code or, for the first owner, the setup key. */
async function login(url: string, email: string, password: string, admission: { code?: string; setupKey?: string } = {}, scopes = ["read", "write", "publish"], verifyFrom?: string): Promise<string> {
  if (verifyFrom && !isLoopbackUrl(url)) throw Error(`--verify-from reads a development mailbox on this machine, so it works only with a Kernel on localhost or 127.0.0.1, not ${new URL(url).host}.`);
  let response = await fetch(new URL("/_konae/owner/login", url), {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, password }),
  });
  const seen = verifyFrom ? await mailboxLines(verifyFrom) : 0;
  if (!response.ok && (admission.code || admission.setupKey)) response = await fetch(new URL("/_konae/owner/signup", url), {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, password, ...admission }),
  });
  if (response.status === 202 && verifyFrom) response = await verifyFromMailbox(url, verifyFrom, email, password, seen);
  if (response.status === 202) throw Error(VERIFY_BY_LINK);
  if (!response.ok) throw Error((await response.json()).error ?? "Owner sign-in failed.");
  const cookie = response.headers.get("set-cookie")?.split(";")[0];
  if (!cookie) throw Error("Owner sign-in returned no session.");
  const client = new McpClient(endpoint(url), { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  const token = await client.call<{ token: string }>("token_create", { scopes, ttlSeconds: 86400 });
  await fetch(new URL("/_konae/owner/logout", url), { method: "POST", headers: { cookie } });
  return token.token;
}

async function credentials(root: string) {
  if (process.env.KONAE_MCP_TOKEN) return process.env.KONAE_MCP_TOKEN;
  return (await json<{ token: string }>(path.join(root, ".konae/credentials.json"))).token;
}

/** Files the CLI writes beside the Capsule, which are never uploaded as its source. */
const generatedFiles = new Set(["konae.json", ACTION_CONTRACT_FILE, ACTION_CONTRACT_SCHEMA_FILE]);

async function sources(root: string, prefix = ""): Promise<Record<string, string>> {
  const result: Record<string, string> = {};
  for (const entry of await readdir(path.join(root, prefix), { withFileTypes: true })) {
    if (entry.name.startsWith(".") || ["node_modules", "dist", "data", "target"].includes(entry.name)) continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) Object.assign(result, await sources(root, relative));
    else if (entry.isFile() && /\.(?:html|css|js|jsx|ts|tsx|json|sql)$/.test(entry.name) && !generatedFiles.has(relative)) result[relative] = await readFile(path.join(root, relative), "utf8");
  }
  return result;
}

/**
 * Every file the build may publish, text or binary. Dot folders and
 * node_modules are skipped at any depth, and the folders the CLI and the
 * Development Kernel write (dist, data, target) at the Capsule root.
 */
async function capsulePaths(root: string, prefix = ""): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(path.join(root, prefix), { withFileTypes: true })) {
    if (entry.name.startsWith(".") || entry.name === "node_modules") continue;
    if (!prefix && ["dist", "data", "target"].includes(entry.name)) continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) result.push(...await capsulePaths(root, relative));
    else if (entry.isFile() && !generatedFiles.has(relative)) result.push(relative);
  }
  return result;
}

/** A file as text when it is UTF-8, else as bytes, which the artifact carries as base64. */
async function readCapsuleFile(root: string, relative: string): Promise<string | Uint8Array> {
  const bytes = new Uint8Array(await readFile(path.join(root, relative)));
  try { return new TextDecoder("utf-8", { fatal: true }).decode(bytes); } catch { return bytes; }
}

async function bundleModule(root: string, entry: string) {
  // absWorkingDir names each bundled module by its Capsule path, which the build then drops.
  const result = await build({ entryPoints: [path.join(root, entry)], absWorkingDir: path.resolve(root), bundle: true, write: false, platform: "browser", format: "esm", target: "es2022", outdir: "out" });
  const script = result.outputFiles.find((file) => file.path.endsWith(".js"))?.text;
  if (script === undefined) throw Error("The local builder produced no JavaScript.");
  return { script, styles: result.outputFiles.find((file) => file.path.endsWith(".css"))?.text ?? "" };
}

/**
 * Builds the Capsule in `root` by the static publishing contract in
 * src/lib/static-artifact.js, the rules the editor's build runs too.
 */
export async function buildLocal(root: string) {
  const files = await sources(root);
  const config = await json<Partial<Config>>(path.join(root, "konae.json")).catch(() => undefined);
  const sdk = await readFile(path.join(toolRoot, "src/mcp/runtime.js"), "utf8");
  const artifact = await assembleStaticArtifact({
    paths: await capsulePaths(root),
    read: (relative) => readCapsuleFile(root, relative),
    compile: async () => {
      const { script, styles } = await bundleModule(root, files["src/main.ts"] !== undefined ? "src/main.ts" : "app.js");
      return { script, styles: [files["styles.css"] ?? "", styles].join("\n") };
    },
    bundle: (relative) => bundleModule(root, relative),
    runtime: sdk,
    routing: config?.project?.manifest?.routing,
    warn: (message) => console.warn(`Warning: ${message}`),
  });
  await mkdir(path.join(root, ".konae"), { recursive: true });
  await writeFile(path.join(root, ".konae/artifact.json"), JSON.stringify(artifact));
  return { artifact, files };
}

/**
 * Builds and publishes. On a first release `project.manifest.signupPolicy`
 * goes as `initialSignupPolicy`; on a later one the Kernel keeps the app's
 * policy and `warn` gets one line when konae.json names another.
 */
export async function publishLocal(root: string, config: Config, token: string, warn: (message: string) => void = console.warn, pulled?: Pulled) {
  if (!config.url) throw Error(NO_INSTALLATION);
  const client = new McpClient(endpoint(config.url), { token });
  const wanted = manifestSignupPolicy(config.project.manifest);
  // The policy is not part of the Capsule manifest, so the Kernel's project record never holds an allowlist.
  const { signupPolicy: _, ...manifest } = config.project.manifest;
  const project = { ...config.project, manifest };
  await client.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest });
  const remote = await client.call<{ deployment?: { id: string } | null }>("project_inspect", { projectId: project.id });
  const first = !remote.deployment;
  const { artifact, files } = await buildLocal(root);
  // Only a project without a Deployment takes a package, so a pulled copy goes with the first release or not at all.
  const components = first && pulled ? await uploadPulled(client, root, pulled, project.id) : undefined;
  const payload = {
    project: { ...project, files }, visibility: manifest.visibility ?? "private", artifact,
    ...(first && wanted ? { initialSignupPolicy: wanted } : {}),
    ...(components ? { visibility: pulled!.manifest.visibility, initialPackage: { manifest: pulled!.manifest, components } } : {}),
  };
  let receipt: { id: string; url: string };
  let transfer: Transfer | undefined;
  try {
    transfer = await client.upload(new TextEncoder().encode(JSON.stringify(payload)), project.id);
    const plan = await client.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    await writeFile(path.join(root, ".konae/release-plan.json"), JSON.stringify(plan, null, 2));
    receipt = await client.call<{ id: string; url: string }>("release_apply", { planId: plan.planId });
    await writeFile(path.join(root, ".konae/deployment.json"), JSON.stringify(receipt, null, 2));
  } finally {
    for (const id of Object.values(components ?? {})) await client.cancel(id).catch(() => {});
    if (transfer) try { await client.cancel(transfer.transferId); } catch (error) { console.warn(`Could not release the upload: ${error instanceof Error ? error.message : error}`); }
  }
  if (!first && wanted) {
    // Reading the policy needs only read. A Kernel that cannot answer leaves the publish as it is.
    const current = await client.call<{ mode: SignupMode; allowlist?: string[] }>("signup_policy_read", { deploymentId: receipt.id }).catch(() => undefined);
    const warning = current?.mode && signupPolicyWarning(wanted, { mode: current.mode, allowlist: current.allowlist ?? [] });
    if (warning) warn(warning);
  }
  return Object.assign(receipt, { first });
}

export async function exportLocal(root: string, config: Config, output: string, token?: string) {
  const { artifact, files } = await buildLocal(root);
  let manifest = config.project.manifest;
  let captured: { package: PackageManifest; files: FileTarEntry[] } | undefined;
  const scratch = token === undefined ? undefined : await mkdtemp(path.join(tmpdir(), "konae-package-"));
  try {
    if (token !== undefined && scratch) {
      if (!config.url) throw Error(NO_INSTALLATION);
      const client = new McpClient(endpoint(config.url), { token });
      const remote = await client.call<{ deployment?: { id: string; manifest?: Record<string, unknown> } }>("project_inspect", { projectId: config.project.id });
      if (!remote.deployment) throw Error("Publish the project before exporting its live data, or omit --with-data.");
      const deployedManifest = remote.deployment.manifest;
      if (deployedManifest?.visibility !== "public" && deployedManifest?.visibility !== "private") {
        throw Error("The deployment did not provide its visibility. Live data export stopped.");
      }
      manifest = deployedManifest;
      const downloaded = await downloadPackage(client, remote.deployment.id, scratch);
      const checked = downloaded.manifest;
      const components: FileTarEntry[] = downloaded.files.map(({ name, file, size }) => ({ path: `konae/package/${packageComponentFiles[name as keyof typeof packageComponentFiles]}`, file, size }));
      captured = { package: checked, files: components };
    }
    const [{ projectExportEntries }, { kernelFiles }] = await Promise.all([
      import("../src/lib/export-project-kit"), import("../src/lib/export-project-sources"),
    ]);
    const project = {
      ...config.project,
      manifest: normalizeManifest({ ...manifest, visibility: manifest.visibility ?? "private" }),
      files, directories: [], updatedAt: Date.now(),
    };
    const entries = projectExportEntries(project, artifact, kernelFiles, captured && { package: captured.package });
    // The kit rebuilds from app/, so it also needs the files that are not
    // source text: images, fonts and everything under public/.
    for (const relative of await capsulePaths(root)) {
      if (files[relative] === undefined) entries.push({ path: `app/${relative}`, contents: new Uint8Array(await readFile(path.join(root, relative))) });
    }
    await writeTarFile(output, [...entries, ...(captured?.files ?? [])]);
    return output;
  } finally {
    if (scratch) await rm(scratch, { recursive: true, force: true });
  }
}

/**
 * Writes the Action Contract of the Capsule in `root` into `out`: the JSON,
 * its schema and the Markdown. The project's ID and name come from
 * konae.json when there is one.
 */
export async function writeContract(root: string, out = root) {
  const read = async (file: string) => readFile(path.join(root, file), "utf8");
  let project: { id: string; name: string } | undefined;
  try {
    const config = await json<Config>(path.join(root, "konae.json"));
    project = { id: config.project.id, name: config.project.name };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  let actionsSource: string;
  try { actionsSource = await read("actions.json"); } catch { throw Error(`No actions.json in ${root}.`); }
  const schemaSource = await read("schema.sql").catch(() => "");
  const { contract, warnings } = generateActionContract({ actionsSource, schemaSource, project });
  await mkdir(out, { recursive: true });
  await writeFile(path.join(out, ACTION_CONTRACT_FILE), serializeActionContract(contract));
  await writeFile(path.join(out, ACTION_CONTRACT_SCHEMA_FILE), `${JSON.stringify(actionContractSchema, null, 2)}\n`);
  await writeFile(path.join(out, ACTION_CONTRACT_MARKDOWN), renderActionContractMarkdown(contract));
  return { contract, warnings, file: path.join(out, ACTION_CONTRACT_FILE) };
}

/** Add each entry to .gitignore unless a line already names it. */
export async function ignoreInGit(root: string, entries: string[]) {
  const file = path.join(root, ".gitignore");
  let current = "";
  try { current = await readFile(file, "utf8"); } catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
  const present = new Set(current.split(/\r?\n/).map((line) => line.trim().replace(/^\//, "").replace(/\/$/, "")));
  const missing = entries.filter((entry) => !present.has(entry.replace(/\/$/, "")));
  if (missing.length === 0) return;
  const separator = current && !current.endsWith("\n") ? "\n" : "";
  await writeFile(file, `${separator}${missing.join("\n")}\n`, { flag: "a" });
}

function slugOf(name: string) {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "app";
}

/**
 * Writes a template and konae.json into an empty folder, without a Kernel.
 * The project ID is made here; the first publish registers it on the
 * installation with project_create, which takes an ID the caller chose.
 */
export async function initLocal(root: string, options: { name?: string; template?: string; url?: string } = {}) {
  // The file project_create answers from, so a folder starts the same through the CLI and through MCP.
  const all = await json<ProjectTemplates>(path.join(toolRoot, PROJECT_TEMPLATES_FILE));
  const born = all.templates[options.template ?? all.default];
  if (!born) throw Error(`Unknown template ${options.template}. Choose one of: ${INIT_TEMPLATES.join(", ")}.`);
  await assertEmptyForInit(root);
  const name = options.name ?? "My app";
  for (const [filename, source] of Object.entries(born.files)) {
    const target = path.resolve(root, filename);
    if (!target.startsWith(path.resolve(root) + path.sep)) throw Error("Invalid template path.");
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, source, { flag: "wx" });
  }
  // The template says who may open the app. Only people the owner invites can create an account.
  const manifest = { version: 1, visibility: born.visibility, signupPolicy: INIT_SIGNUP_POLICY };
  const config: Config = { ...(options.url ? { url: options.url } : {}), project: { id: `app_${crypto.randomUUID()}`, name, slug: slugOf(name), manifest } };
  await writeFile(path.join(root, "konae.json"), `${JSON.stringify(config, null, 2)}\n`, { flag: "wx" });
  await ignoreInGit(root, [".konae/", "node_modules/"]);
  return config;
}

/**
 * Sets, on a Development Kernel, each signing secret actions.json names and
 * the Kernel does not hold, to a value made here, and returns one line per
 * secret it set. A release that signs with an unset secret is refused, and
 * the published app's values never leave its installation.
 */
export async function setDevSecrets(root: string, client: Pick<McpClient, "call">, project: { id: string; name: string; slug: string; manifest: Record<string, unknown> }) {
  let actions: Record<string, { http?: { signature?: { secret?: string } }; auth?: { secret?: string } }>;
  try { actions = JSON.parse(await readFile(path.join(root, "actions.json"), "utf8")).actions ?? {}; } catch { return []; }
  const wanted = new Map<string, "route" | "call">();
  for (const action of Object.values(actions)) {
    if (typeof action?.http?.signature?.secret === "string") wanted.set(action.http.signature.secret, "route");
    if (typeof action?.auth?.secret === "string") wanted.set(action.auth.secret, "call");
  }
  if (wanted.size === 0) return [];
  await client.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
  const held = new Set((await client.call<{ secrets?: { name: string }[] }>("secrets_list", { projectId: project.id })).secrets?.map((secret) => secret.name));
  const lines: string[] = [];
  for (const [name, use] of wanted) {
    if (held.has(name)) continue;
    // whsec_ and base64, so a svix route takes it too.
    const value = `whsec_${Buffer.from(crypto.getRandomValues(new Uint8Array(24))).toString("base64")}`;
    await client.call("secret_set", { projectId: project.id, name, value, use });
    lines.push(`Set the secret ${name} on this Development Kernel to ${value}. Sign local requests with it; the published app keeps its own.`);
  }
  return lines;
}

/** init writes a template into the directory, so it must hold nothing but dotfiles (.git, .konae, .gitignore). */
async function assertEmptyForInit(root: string) {
  let entries: string[] = [];
  try { entries = await readdir(root); } catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
  const occupied = entries.filter((name) => !name.startsWith("."));
  if (occupied.length > 0) throw Error(`Refusing to overwrite ${occupied.slice(0, 3).join(", ")}. Choose an empty directory.`);
}

type DomainView = {
  hostname: string | null; status: "none" | "pending" | "active"; url?: string; generatedUrl: string;
  record?: { type: "TXT"; name: string; value: string }; dns?: string; rules?: DomainRules;
};

/** domain_read, domain_set or domain_clear, with the hostname checked by the Kernel's rules first. Returns the lines to print. */
export async function domainLocal(client: Pick<McpClient, "call">, deploymentId: string, request: { set?: string; clear?: boolean }): Promise<string[]> {
  if (request.clear) {
    const cleared = await client.call<{ cleared: string | null; generatedUrl: string }>("domain_clear", { deploymentId });
    return [cleared.cleared ? `${cleared.cleared} no longer serves this app. ${cleared.generatedUrl} keeps answering.` : `This app had no custom domain. It answers at ${cleared.generatedUrl}.`];
  }
  const current = await client.call<DomainView>("domain_read", { deploymentId });
  let view = current;
  if (request.set !== undefined) {
    const checked = checkHostname(request.set, current.rules ?? { suffix: "", installationHosts: [] });
    if (!checked.ok) throw Error(checked.message);
    view = await client.call<DomainView>("domain_set", { deploymentId, hostname: checked.hostname });
  }
  if (view.status === "none") return [`This app has no custom domain. It answers at ${view.generatedUrl}.`];
  if (view.status === "active") return [`${view.url} serves this app, next to ${view.generatedUrl}. Your proxy must terminate TLS for ${view.hostname} and forward Host unchanged.`];
  return [
    `${view.hostname} does not serve yet.${view.dns ? ` ${view.dns}` : ""}`,
    `Add this record at your DNS provider: ${view.record!.type} ${view.record!.name} "${view.record!.value}"`,
    `Point ${view.hostname} at this installation's proxy, then run this command again with --set ${view.hostname}.`,
  ];
}

async function main() {
  const argv = process.argv.slice(2);
  if (argv[0] === "--") argv.shift();
  const [command, ...args] = argv;
  if (!command || ["help", "--help"].includes(command)) {
    console.log(HELP);
    return;
  }
  if (!["login", "init", "build", "contract", "conform", "export", "publish", "dev", "pull", "sql", "events", "domain"].includes(command)) throw Error(`Unknown command ${command}.\n\n${HELP}`);
  const option = (name: string, fallback?: string) => {
    const i = args.indexOf(`--${name}`);
    if (i < 0) return fallback;
    const value = args[i + 1];
    if (value === undefined || value.startsWith("--")) throw Error(`--${name} needs a value.`);
    return value;
  };
  const root = path.resolve(option("dir", process.cwd())!);
  if (command === "login") {
    const url = option("url", process.env.KONAE_URL ?? "http://localhost:4173")!;
    const email = process.env.KONAE_EMAIL; const password = process.env.KONAE_PASSWORD;
    if (!email || !password) throw Error("Set KONAE_EMAIL and KONAE_PASSWORD. To create the account, also set KONAE_INVITATION_CODE, or KONAE_SETUP_KEY for the first owner.");
    const scopes = loginScopes(args);
    const verifyFrom = option("verify-from");
    const token = await login(url, email, password, { code: process.env.KONAE_INVITATION_CODE, setupKey: process.env.KONAE_SETUP_KEY }, scopes, verifyFrom && path.resolve(verifyFrom));
    await mkdir(path.join(root, ".konae"), { recursive: true });
    await writeFile(path.join(root, ".konae/credentials.json"), JSON.stringify({ url, token }), { mode: 0o600 });
    await chmod(path.join(root, ".konae/credentials.json"), 0o600);
    await ignoreInGit(root, [".konae/"]);
    console.log("Saved a 24-hour MCP credential in .konae/credentials.json. Keep .konae out of Git.");
    return;
  }
  if (command === "init") {
    const config = await initLocal(root, { name: option("name"), template: option("template"), url: option("url", process.env.KONAE_URL) });
    console.log(`Created ${config.project.name} in ${root}. Run it with: bun run konae -- dev --dir ${root}`);
    return;
  }
  if (command === "build") { await buildLocal(root); console.log("Built .konae/artifact.json"); return; }
  if (command === "contract") {
    const { contract, warnings, file } = await writeContract(root, path.resolve(option("out", root)!));
    for (const warning of warnings) console.warn(`Warning: ${warning.message}`);
    console.log(`Wrote the Action Contract for ${contract.actions.length} action${contract.actions.length === 1 ? "" : "s"} to ${file}, with its schema and Markdown.`);
    return;
  }
  if (command === "conform") {
    const { conformFromCli } = await import("./conform");
    process.exitCode = await conformFromCli(args);
    return;
  }
  if (command === "domain") {
    const deploymentId = option("deployment");
    if (!deploymentId) throw Error("Name the Deployment with --deployment <id>. project_inspect shows its ID.");
    const saved = process.env.KONAE_MCP_TOKEN ? undefined : await json<{ url: string; token: string }>(path.join(root, ".konae/credentials.json"));
    const url = option("url", saved?.url ?? process.env.KONAE_URL ?? "http://localhost:4173")!;
    for (const line of await domainLocal(new McpClient(endpoint(url), { token: process.env.KONAE_MCP_TOKEN ?? saved!.token }), deploymentId, { set: option("set"), clear: args.includes("--clear") })) console.log(line);
    return;
  }
  const config = await json<Config>(path.join(root, "konae.json"));
  // The installation: --url, then konae.json, then the one login saved beside the token it would send.
  const installation = async () => {
    const url = option("url") ?? config.url ?? (await json<{ url?: string }>(path.join(root, ".konae/credentials.json")).catch(() => undefined))?.url;
    if (!url) throw Error(NO_INSTALLATION);
    return url;
  };
  if (command === "export") {
    const output = path.resolve(option("out", path.join(root, ".konae/selfhost.tar"))!);
    const withData = args.includes("--with-data");
    await exportLocal(root, withData ? { ...config, url: await installation() } : config, output, withData ? await credentials(root) : undefined);
    console.log(`Exported self-hosted app to ${output}.`);
    return;
  }
  if (command === "publish") {
    const url = await installation();
    console.log((await publishLocal(root, { ...config, url }, await credentials(root))).url);
    // A project init wrote without an installation keeps the first one it was published to.
    if (!config.url) await writeFile(path.join(root, "konae.json"), `${JSON.stringify({ url, ...config }, null, 2)}\n`);
    return;
  }
  // sql and events reach the Development Kernel dev is running for the folder, and the published Deployment only with --prod.
  const target = async (): Promise<{ client: McpClient; deploymentId: string; local: boolean; feed?: string }> => {
    if (!args.includes("--prod")) {
      const session = await json<{ url: string; token: string; deploymentId: string; events?: string }>(path.join(root, DEV_SESSION)).catch(() => undefined);
      if (!session) throw Error(`No Development Kernel is running for this folder. Start one with: bun run konae -- dev --dir ${root}\nTo reach the published app instead, add --prod.`);
      return { client: new McpClient(endpoint(session.url), { token: session.token }), deploymentId: session.deploymentId, local: true, feed: session.events };
    }
    const client = new McpClient(endpoint(await installation()), { token: await credentials(root) });
    const remote = await client.call<{ deployment?: { id: string } | null }>("project_inspect", { projectId: config.project.id });
    if (!remote.deployment) throw Error("This project has no Deployment yet. Publish it first.");
    return { client, deploymentId: remote.deployment.id, local: false };
  };
  if (command === "sql") {
    const file = option("file");
    // The statement is the one argument that is neither a flag nor a flag's value.
    const inline = args.find((arg, i) => !arg.startsWith("--") && !["--dir", "--url", "--file"].includes(args[i - 1]));
    const sql = file ? await readFile(path.resolve(file), "utf8") : inline;
    if (!sql?.trim()) throw Error("Give the statement in quotes or with --file <file.sql>.");
    const { client, deploymentId } = await target();
    console.log(JSON.stringify(await client.call("database_execute", { deploymentId, operation: "sql", input: { sql } }), null, 2));
    return;
  }
  if (command === "events") {
    const { formatEvent, readEvents } = await import("./events");
    const { client, deploymentId, local, feed } = await target();
    const days = Number(option("days", "7"));
    if (!Number.isInteger(days) || days < 1 || days > 400) throw Error("--days takes a whole number from 1 to 400.");
    const asJson = args.includes("--json");
    const seen = new Map<number, number>();
    const printed = new Set<string>();
    const read = async () => {
      // Only a Development Kernel has a mailbox this machine can read.
      // dev's events page stamps each mail when it is written, so its answer is preferred to a read from here.
      const served = feed ? await fetch(`${feed}/events.json?days=${days}`).then((response) => response.ok ? response.json() as Promise<AppEvent[]> : undefined).catch(() => undefined) : undefined;
      const events = served ?? await readEvents(client, deploymentId, { days, seen, mailbox: local ? path.join(root, ".konae/dev-data/mailbox.jsonl") : undefined });
      for (const event of events) {
        // A notice or a call changes state, so the same event prints again when its outcome does.
        const key = `${event.id}:${event.outcome}`;
        if (printed.has(key)) continue;
        printed.add(key);
        console.log(asJson ? JSON.stringify(event) : formatEvent(event));
      }
    };
    await read();
    if (!args.includes("--follow")) {
      if (printed.size === 0 && !asJson) console.log(`Nothing yet in the last ${days} days: no email, call, scheduled run, route or Signal.`);
      return;
    }
    for (;;) { await Bun.sleep(1000); await read().catch((error) => { console.error(error instanceof Error ? error.message : error); process.exit(1); }); }
  }
  if (command === "pull") {
    const url = await installation();
    const pulled = await pullLocal(root, new McpClient(endpoint(url), { token: await credentials(root) }), url, config.project.id);
    console.log(`Saved ${describePulled(pulled)} in ${path.join(root, PULL_DIRECTORY)}. It holds the app's accounts and rows, so keep .konae out of Git.`);
    console.log(`Start the local app from it with: bun run konae -- dev --dir ${root} --reset`);
    return;
  }
  if (command === "dev") {
    const dataDirectory = path.join(root, ".konae/dev-data");
    const devSession = path.join(root, DEV_SESSION);
    if (args.includes("--reset")) await rm(dataDirectory, { recursive: true, force: true });
    await mkdir(dataDirectory, { recursive: true });
    const secretFile = path.join(root, ".konae/dev-secret");
    let secret: string;
    try { secret = await readFile(secretFile, "utf8"); } catch { secret = crypto.randomUUID() + crypto.randomUUID(); await writeFile(secretFile, secret, { mode: 0o600 }); }
    const port = option("port", "8796")!;
    if (!/^\d+$/.test(port) || Number(port) < 1024 || Number(port) > 65535) throw Error("Choose a local port between 1024 and 65535.");
    // KONAE_KERNEL_BIN names a Kernel binary built from these sources, so dev runs without cargo.
    const kernelBinary = process.env.KONAE_KERNEL_BIN ? path.resolve(process.env.KONAE_KERNEL_BIN) : path.join(toolRoot, "kernel-rs/target/debug/konae-kernel");
    if (!process.env.KONAE_KERNEL_BIN) {
      const compilation = Bun.spawn(["cargo", "build", "--manifest-path", path.join(toolRoot, "kernel-rs/Cargo.toml")], { stdout: "inherit", stderr: "inherit" });
      if (await compilation.exited !== 0) throw Error("Could not build the Development Kernel. Install Rust, or set KONAE_KERNEL_BIN to a Kernel binary built from this checkout.");
    }
    const env: Record<string, string | undefined> = { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))), KONAE_BIND: "127.0.0.1", KONAE_DATA_DIR: path.join(root, ".konae/dev-data"), KONAE_ADMIN_KEY: secret, KONAE_KERNEL_PORT: port, KONAE_PUBLIC_ORIGIN: `http://localhost:${port}`, KONAE_HOST_SUFFIX: "localhost", // Account links and notices land in a file on this machine, never a provider.
      KONAE_MAIL_TRANSPORT: "development",
      // Shortens every schedule so a run can be watched; the Kernel takes it only on loopback.
      ...(process.env.KONAE_SCHEDULE_MIN_SECONDS ? { KONAE_SCHEDULE_MIN_SECONDS: process.env.KONAE_SCHEDULE_MIN_SECONDS } : {}) };
    for (const key of Object.keys(env)) if (key.startsWith("KONAE_S3_") || ["KONAE_ARTIFACT_BUCKET", "KONAE_DATA_BUCKET", "KONAE_MCP_ORIGIN"].includes(key)) delete env[key];
    const child = Bun.spawn([kernelBinary], { env, stdout: "inherit", stderr: "inherit" });
    const stop = () => { child.kill(); };
    process.once("SIGINT", stop); process.once("SIGTERM", stop);
    process.once("exit", stop);
    const url = `http://127.0.0.1:${port}`;
    let feed: { url: string; stop(): void } | undefined;
    let ready = false;
    for (let attempt = 0; attempt < 100; attempt++) {
      if (child.exitCode !== null) throw Error("Development Kernel stopped before becoming ready.");
      try { ready = (await fetch(`${url}/_konae/health`)).ok; } catch {}
      if (ready) break; await Bun.sleep(100);
    }
    if (!ready) { child.kill(); throw Error("Development Kernel did not become ready."); }
    // The development Kernel opens signup below, which needs manage_users.
    const token = await login(url, "local-owner@konae.test", secret, { setupKey: secret }, ["read", "write", "publish", "manage_users"]);
    // Development accounts come from signup, so the local app starts Open and a republish never warns.
    const devConfig = { ...config, url, project: { ...config.project, manifest: { ...config.project.manifest, signupPolicy: { mode: "open" } } } };
    try {
      const admin = new McpClient(endpoint(url), { token });
      const pulled = await readPulled(root);
      const { signupPolicy: _, ...manifest } = devConfig.project.manifest;
      let first: { id: string; url: string; first: boolean };
      const secrets = async () => { for (const line of await setDevSecrets(root, admin, { ...devConfig.project, manifest })) console.log(line); };
      try {
        await secrets();
        first = await publishLocal(root, devConfig, token, console.warn, pulled);
      } catch (error) {
        // A refused first release leaves no Deployment. When the refusal is a schema the pulled copy
        // does not have yet, install the copy alone and keep serving until it is migrated.
        const schemaBehind = error instanceof Error && error.message.includes("does not match the deployed schema");
        const remote = pulled && schemaBehind ? await admin.call<{ deployment?: unknown }>("project_inspect", { projectId: config.project.id }).catch(() => undefined) : undefined;
        if (!pulled || !remote || remote.deployment) throw error;
        first = { ...await installPulledAlone(admin, root, pulled, { ...devConfig.project, manifest }), first: true };
        console.error(`${error instanceof Error ? error.message : error}\nThe pulled copy is installed without the app. Change its database the way the published one must change, for example: bun run konae -- sql --dir ${root} "ALTER TABLE ..." Then save a file and dev publishes the app again.`);
      }
      // The marker sits in dev-data, so --reset removes it with the data it describes.
      const marker = path.join(dataDirectory, "pulled.json");
      if (pulled && first.first) {
        await writeFile(marker, JSON.stringify({ packageId: pulled.manifest.packageId }));
        console.log(`Started from ${describePulled(pulled)}. Its sessions did not travel and its API keys arrive revoked. Its accounts keep their passwords; to sign in as one without knowing it, ask the app for a password reset and open the link in ${path.join(dataDirectory, "mailbox.jsonl")}.`);
      } else if (pulled && (await json<{ packageId?: string }>(marker).catch(() => undefined))?.packageId !== pulled.manifest.packageId) {
        console.log(`A pulled copy this Development Kernel did not start from waits in ${path.join(root, PULL_DIRECTORY)}. Run dev with --reset to replace the local data with it.`);
      }
      // The events page answers on the next port, on this machine only. A taken port leaves the command, which reads the Kernel itself.
      const { serveEvents } = await import("./events");
      const eventsPort = Number(option("events-port", String(Number(port) + 1)));
      try { feed = serveEvents(admin, first.id, path.join(dataDirectory, "mailbox.jsonl"), eventsPort); }
      catch { console.warn(`Port ${eventsPort} is taken, so the events page is off. Choose another with --events-port, or run events.`); }
      // What sql and events read: this Kernel, its owner token, the Deployment and the events page.
      await writeFile(devSession, JSON.stringify({ url, token, deploymentId: first.id, ...(feed ? { events: feed.url } : {}) }), { mode: 0o600 });
      // dev-data kept from an earlier run, or a pulled copy, may hold another policy. The development
      // Kernel is isolated and local, so let the developer create test accounts.
      await admin.call("signup_policy_set", { deploymentId: first.id, mode: "open" });
      console.log(first.url);
      console.log(feed
        ? `Email, calls, scheduled runs and Signals of this app: ${feed.url} in a browser, or bun run konae -- events --dir ${root} --json`
        : `See the email, calls, scheduled runs and Signals of this app with: bun run konae -- events --dir ${root} --follow`);
      let working = false; let queued = false;
      const rebuild = async () => {
        if (working) { queued = true; return; } working = true;
        do { queued = false; try { await secrets(); console.log(`Updated ${(await publishLocal(root, devConfig, token)).url}. Reload the app to see changes.`); } catch (e) { console.error(e instanceof Error ? e.message : e); } } while (queued);
        working = false;
      };
      let timer: ReturnType<typeof setTimeout>;
      const watcher = watch(root, { recursive: true }, (_, filename) => {
        if (!filename || filename.startsWith(".") || filename.includes("node_modules") || !/\.(?:html|css|js|jsx|ts|tsx|json|sql)$/.test(filename)) return;
        clearTimeout(timer); timer = setTimeout(() => void rebuild(), 200);
      });
      await child.exited; watcher.close();
    } finally { child.kill(); feed?.stop(); await rm(devSession, { force: true }); }
    return;
  }
}

if (import.meta.main) main().catch(error => { console.error(error.message); process.exitCode = 1; });
