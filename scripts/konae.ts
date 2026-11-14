#!/usr/bin/env bun
import { chmod, cp, mkdir, mkdtemp, readFile, readdir, readlink, rm, stat, symlink, writeFile } from "node:fs/promises";
import { existsSync, watch } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { createInterface } from "node:readline/promises";
import { McpClient } from "../src/mcp/client";
import { packageComponentFiles, type PackageComponentName } from "../src/lib/portable-package";
import {
  DATA_FOLDER, DIST_FOLDER, KERNEL_VERSION, SETTINGS_FILE, SOURCES_FOLDER, dataEntries, generatedFiles, gitignore,
  type AppSettings, type FolderEntry,
} from "../src/lib/app-folder";
import { describeSchemaChange, publishProject, type ConfirmSchema } from "../src/deployments/release";
import { PROJECT_TEMPLATES_FILE, type ProjectTemplates } from "./project-templates";
import type { AppEvent, ReadRecord } from "./events";
import { PULL_DIRECTORY, describePulled, downloadData, pullLocal, readPulled, type Pulled } from "./pull";
import {
  ACTION_CONTRACT_FILE, ACTION_CONTRACT_MARKDOWN, ACTION_CONTRACT_SCHEMA_FILE,
  actionContractSchema, generateActionContract, renderActionContractMarkdown, serializeActionContract,
  type ActionContract,
} from "../src/lib/action-contract";
import { checkHostname, type DomainRules } from "../src/lib/domain-name";
import { assembleStaticArtifact } from "../src/lib/static-artifact.js";
import { ACTION_TYPES_FILE, renderActionTypes } from "../src/lib/action-types";

const toolRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
type Project = { id: string; name: string; slug: string; manifest: Record<string, unknown>; files?: Record<string, string> };
/** `url` is absent in a project that `init` wrote without one, until a publish names the installation. */
/** What a publish sends. `contactEmail` is where notices to the owner go, from konae.json. */
type Config = { url?: string; project: Project; contactEmail?: string };
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
  return `konae.json asks for signup ${describePolicy(wanted)}, but this app has ${describePolicy(current)}. A publish sets the policy only on the first release and never changes who may join afterwards. Change it in the editor's Access > Users view, or call signup_policy_set with a credential that holds manage_users.`;
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

/** Where a running `dev` leaves the address of its events page for `events`. */
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
init      --dir <empty folder> [--name <name>] [--template <id>]
          Writes an app folder: konae.json, the template in app/, AGENTS.md, action-contract.md,
          compose.yaml, .env.example and package.json, whose npm run build writes dist/. It needs no
          credential and no Kernel, and the first publish registers the project.
          Templates: ${INIT_TEMPLATES.join(", ")}.
dev       --dir <folder> [--port 8796] [--events-port 8797] [--reset] [--confirm-schema <confirmation>]
          Builds dist/ and runs the kernel binary on the folder, with data/ as its data, and starts it
          again when a file in app/ changes. The Kernel checks the folder as a release. Its email,
          notices and invitations included, goes to <folder>/.konae/kernel/mailbox.jsonl.
          It sets and prints a local value for each signing secret actions.json names, and serves a
          page of what the app did (email, calls, scheduled runs, Signals) on the next port.
          KONAE_KERNEL_BIN names a built kernel binary to run instead of compiling one.
          --reset imports the data pull exported into data/, after a backup, or empties data/ when there is none.
          A schema change that loses data is refused; --confirm-schema applies the plan it names.
          Writes actions.d.ts, which types miniapp.actions.call.
sql       --dir <folder> [--prod] [--url <Kernel URL>] ("<statement>" | --file <file.sql>)
          Runs SQL on data/database.sqlite with the kernel binary's sql command. Only --prod reaches the
          published Deployment, through database_execute.
events    --dir <folder> [--follow] [--json] [--days 7] [--prod]
          Lists what the app did besides answer its page: email, calls to other services, scheduled
          runs, routes and Signals, oldest first. Locally an email shows its recipient and text,
          because nothing is sent. --json prints one object per line and --follow keeps printing.
          Only --prod reads the published Deployment.
pull      --dir <folder> [--url <Kernel URL>]
          Exports the published app's data (database, accounts, files, Usage) into
          <folder>/.konae/pull/data. Needs a credential from login --export-data. dev --reset imports it.
build     --dir <folder>                 Builds app/ into dist/. It runs npm run build once node_modules is
          installed, and the built-in bundler before that.
contract  --dir <folder> [--out <folder>]   Writes action-contract.json, its schema, action-contract.md and
          actions.d.ts, which types miniapp.actions.call.
publish   --dir <folder> [--url <Kernel URL>] [--confirm-schema <confirmation>]
          Builds and publishes to --url or the installation login saved. Set each signing secret
          with tool secret_set first. The Kernel changes the database to what schema.sql states. A
          change that drops a table or a column, or changes a column's type, prints its plan and
          asks; without a terminal, pass the plan's confirmation.
conform   --url <app URL> --contract <file> [--token <app session>] [--key <API key>]
          [--secret <name>=<value>] [--inputs <file>]
          Calls every action and checks the answers against the contract. It runs each mutation once,
          so point it at a test Deployment.
export    --dir <folder> --out <empty folder> [--with-data]
          Writes the app folder: konae.json, app/, a fresh dist/ and the files every app folder
          carries. --with-data adds the published app's data/ and needs a credential from
          login --export-data. Secrets never go with it.
domain    --deployment <id> [--set <hostname> | --clear] [--dir <folder>] [--url <Kernel URL>]
          Shows, sets or clears the app's custom domain. --set prints the TXT record that proves you
          control the hostname; run it again once the record is at your DNS provider.
tool      <name> ['<json arguments>'] [--stdin <key>] [--dir <folder>] [--url <Kernel URL>]
          Calls one tool of the owner's MCP and prints its answer as JSON. The arguments are one JSON
          object and default to {}. --stdin <key> reads that argument from standard input, so a
          secret's value stays off the command line. A refusal prints the tool's own words.

init, build, contract, dev, and sql and events without --prod need no credential. Every other command but login reads it from
<folder>/.konae/credentials.json or KONAE_MCP_TOKEN, and talks to --url or the installation login saved in that file.`;

function endpoint(url: string) { return new URL("/mcp", url).href; }
async function json<T>(filename: string): Promise<T> { return JSON.parse(await readFile(filename, "utf8")); }

/** What login says when the account it created waits for its emailed link. */
export const VERIFY_BY_LINK = "This installation verifies email, so the account waits for the link sent to your address. Open it, choose KONAE_PASSWORD as the password, then run login again with the same KONAE_EMAIL and KONAE_PASSWORD and without the code or key, which are used now. On a Kernel on this machine, run login with --verify-from <its data folder>/mailbox.jsonl to do both in one step.";

/** Whether a Kernel URL names this machine, the only place a development mailbox can be read. */
export function isLoopbackUrl(url: string) {
  const host = new URL(url).hostname.replace(/^\[|\]$/g, "");
  return host === "localhost" || host.endsWith(".localhost") || host === "::1" || /^127(\.\d{1,3}){3}$/.test(host);
}

/**
 * The newest owner verification token for `email` among the development
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

/** The folder's sources: `app/`. */
export function appDir(root: string) { return path.join(root, SOURCES_FOLDER); }

/** The text files of `app/`, which a release carries as the project's files. */
async function sources(root: string, prefix = ""): Promise<Record<string, string>> {
  const result: Record<string, string> = {};
  for (const entry of await readdir(path.join(appDir(root), prefix), { withFileTypes: true })) {
    if (entry.name.startsWith(".") || entry.name === "node_modules") continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) Object.assign(result, await sources(root, relative));
    else if (entry.isFile() && /\.(?:html|css|js|jsx|ts|tsx|json|sql)$/.test(entry.name)) result[relative] = await readFile(path.join(appDir(root), relative), "utf8");
  }
  return result;
}

/** Every file of `app/` the build may publish, text or binary. Dot folders and node_modules are skipped at any depth. */
async function capsulePaths(root: string, prefix = ""): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(path.join(appDir(root), prefix), { withFileTypes: true })) {
    if (entry.name.startsWith(".") || entry.name === "node_modules") continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) result.push(...await capsulePaths(root, relative));
    else if (entry.isFile()) result.push(relative);
  }
  return result;
}

/** Reads konae.json. */
export async function readSettings(root: string): Promise<AppSettings> {
  try { return await json<AppSettings>(path.join(root, SETTINGS_FILE)); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") throw Error(`No konae.json in ${root}. Run init there, or name the app folder with --dir.`);
    throw error;
  }
}

/** The project a release names, from konae.json. The Signup Policy rides in the manifest until the release reads it. */
export function configOf(settings: AppSettings, url?: string): Config {
  return {
    ...(url ? { url } : {}),
    ...(settings.contactEmail ? { contactEmail: settings.contactEmail } : {}),
    project: { id: settings.id, name: settings.name, slug: settings.slug, manifest: { ...settings.manifest, visibility: settings.visibility, signupPolicy: settings.signupPolicy } },
  };
}

/** A file as text when it is UTF-8, else as bytes, which the artifact carries as base64. */
async function readCapsuleFile(root: string, relative: string): Promise<string | Uint8Array> {
  const bytes = new Uint8Array(await readFile(path.join(appDir(root), relative)));
  try { return new TextDecoder("utf-8", { fatal: true }).decode(bytes); } catch { return bytes; }
}

async function bundleModule(root: string, entry: string) {
  // absWorkingDir names each bundled module by its Capsule path, which the build then drops.
  const result = await build({ entryPoints: [path.join(appDir(root), entry)], absWorkingDir: path.resolve(appDir(root)), bundle: true, write: false, platform: "browser", format: "esm", target: "es2022", outdir: "out", jsx: "automatic" });
  const script = result.outputFiles.find((file) => file.path.endsWith(".js"))?.text;
  if (script === undefined) throw Error("The local builder produced no JavaScript.");
  return { script, styles: result.outputFiles.find((file) => file.path.endsWith(".css"))?.text ?? "" };
}

/** True when the app folder's package.json names a build script that writes dist/ and its packages are installed. */
async function hasPackageBuild(root: string) {
  try {
    const manifest = JSON.parse(await readFile(path.join(root, "package.json"), "utf8")) as { scripts?: Record<string, string> };
    if (typeof manifest.scripts?.build !== "string") return false;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
  return (await stat(path.join(root, "node_modules")).catch(() => undefined))?.isDirectory() ?? false;
}

/** Packs an existing dist/ into the artifact shape a release uploads. */
async function artifactFromDist(root: string, prefix = ""): Promise<{ files: Record<string, string>; encodings: Record<string, string> }> {
  const files: Record<string, string> = {};
  const encodings: Record<string, string> = {};
  const dist = path.join(root, DIST_FOLDER);
  for (const entry of await readdir(path.join(dist, prefix), { withFileTypes: true })) {
    if (entry.name.startsWith(".")) continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) {
      const nested = await artifactFromDist(root, relative);
      Object.assign(files, nested.files);
      Object.assign(encodings, nested.encodings);
      continue;
    }
    const bytes = new Uint8Array(await readFile(path.join(dist, relative)));
    try {
      files[relative] = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      files[relative] = Buffer.from(bytes).toString("base64");
      encodings[relative] = "base64";
    }
  }
  return { files, encodings };
}

/**
 * Builds `app/` into `dist/` by the static publishing contract in
 * src/lib/static-artifact.js, the rules the editor's build runs too.
 * When the folder has package.json with a build script (from init) and a
 * node_modules folder, that script writes dist/ and this packs it. Otherwise
 * the built-in bundler runs, so a folder nobody ran npm install in builds too.
 */
export async function buildLocal(root: string) {
  const files = await sources(root);
  if (await hasPackageBuild(root)) {
    const build = Bun.spawn(["npm", "run", "build"], { cwd: root, stdout: "inherit", stderr: "inherit" });
    if ((await build.exited) !== 0) throw Error("npm run build failed.");
    const packed = await artifactFromDist(root);
    if (!packed.files["index.html"]) throw Error("npm run build wrote no dist/index.html.");
    const artifact = { version: 1 as const, entrypoint: "index.html", files: packed.files, encodings: Object.keys(packed.encodings).length ? packed.encodings : undefined };
    return { artifact, files };
  }
  const settings = await readSettings(root).catch(() => undefined);
  const sdk = await readFile(path.join(toolRoot, "src/mcp/runtime.js"), "utf8");
  const entry = files["src/main.tsx"] !== undefined ? "src/main.tsx" : files["src/main.ts"] !== undefined ? "src/main.ts" : "app.js";
  const artifact = await assembleStaticArtifact({
    paths: await capsulePaths(root),
    read: (relative) => readCapsuleFile(root, relative),
    compile: async () => {
      const { script, styles } = await bundleModule(root, entry);
      return { script, styles: [files["styles.css"] ?? "", styles].join("\n") };
    },
    bundle: (relative) => bundleModule(root, relative),
    runtime: sdk,
    routing: settings?.manifest?.routing as "pages" | "spa" | undefined,
    warn: (message) => console.warn(`Warning: ${message}`),
  });
  await writeDist(root, artifact);
  return { artifact, files };
}

/**
 * Builds and publishes. On a first release `project.manifest.signupPolicy`
 * goes as `initialSignupPolicy`; on a later one the Kernel keeps the app's
 * policy and `warn` gets one line when konae.json names another.
 */
export async function publishLocal(root: string, config: Config, token: string, warn: (message: string) => void = console.warn, confirm: ConfirmSchema = keepData) {
  if (!config.url) throw Error(NO_INSTALLATION);
  const client = new McpClient(endpoint(config.url), { token });
  const wanted = manifestSignupPolicy(config.project.manifest);
  // The policy is not part of the Capsule manifest, so the Kernel's project record never holds an allowlist.
  const { signupPolicy: _, ...manifest } = config.project.manifest;
  const project = { ...config.project, manifest };
  let first = false;
  const { plan, receipt } = await publishProject<{ id: string; url: string }>(client, project, async () => {
    // project_inspect needs the project that project_create registers on a first publish.
    const remote = await client.call<{ deployment?: { id: string } | null }>("project_inspect", { projectId: project.id });
    first = !remote.deployment;
    const { artifact, files } = await buildLocal(root);
    return {
      project: { ...project, files }, visibility: manifest.visibility ?? "private", artifact,
      initialSignupPolicy: first ? wanted : undefined, contactEmail: config.contactEmail,
    };
  }, confirm, (error) => console.warn(`Could not release the upload: ${error instanceof Error ? error.message : error}`));
  await mkdir(path.join(root, ".konae"), { recursive: true });
  await writeFile(path.join(root, ".konae/release-plan.json"), JSON.stringify(plan, null, 2));
  await writeFile(path.join(root, ".konae/published.json"), JSON.stringify(receipt, null, 2));
  if (!first && wanted) {
    // Reading the policy needs only read. A Kernel that cannot answer leaves the publish as it is.
    const current = await client.call<{ mode: SignupMode; allowlist?: string[] }>("signup_policy_read", { deploymentId: receipt.id }).catch(() => undefined);
    const warning = current?.mode && signupPolicyWarning(wanted, { mode: current.mode, allowlist: current.allowlist ?? [] });
    if (warning) warn(warning);
  }
  return Object.assign(receipt, { first });
}

/** Keeps the data, so a schema change that loses some is not applied. */
async function keepData() { return false; }

/**
 * Asks on the terminal before a schema change that loses data. The plan is
 * printed with its confirmation; `--confirm-schema <confirmation>` answers
 * yes to that plan only, and without a terminal the answer is no.
 */
export function confirmOnTerminal(accepted?: string): ConfirmSchema {
  return async (change) => {
    for (const line of describeSchemaChange(change)) console.log(line);
    if (accepted !== undefined) return accepted === change.confirmation;
    if (!process.stdin.isTTY) {
      console.log(`To apply it, run the command again with --confirm-schema ${change.confirmation}`);
      return false;
    }
    const prompt = createInterface({ input: process.stdin, output: process.stdout });
    try { return /^y(?:es)?$/i.test((await prompt.question("Apply this change? [y/N] ")).trim()); } finally { prompt.close(); }
  };
}

/** Replaces `dist/` with the built artifact's files. */
async function writeDist(root: string, artifact: { files: Record<string, string>; encodings?: Record<string, string> }) {
  const dist = path.join(root, DIST_FOLDER);
  await rm(dist, { recursive: true, force: true });
  for (const [file, contents] of Object.entries(artifact.files)) {
    const target = path.join(dist, file);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, artifact.encodings?.[file] === "base64" ? Buffer.from(contents, "base64") : contents);
  }
}

/** The files beside app/, dist/ and data/ that every app folder carries. */
async function folderFiles(root: string, settings: AppSettings) {
  const read = (file: string) => readFile(path.join(appDir(root), file), "utf8").catch(() => "");
  return generatedFiles({
    settings, actionsSource: await read("actions.json"), schemaSource: await read("schema.sql"),
    runtime: await readFile(path.join(toolRoot, "src/mcp/runtime.js"), "utf8"),
    staticArtifact: await readFile(path.join(toolRoot, "src/lib/static-artifact.js"), "utf8"),
  });
}

async function writeEntries(root: string, entries: FolderEntry[]) {
  for (const entry of entries) {
    const target = path.resolve(root, entry.path);
    if (!target.startsWith(path.resolve(root) + path.sep)) throw Error(`Invalid folder path ${entry.path}.`);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, entry.contents);
  }
}

/**
 * Exports the app folder into the empty folder `output`: konae.json, app/,
 * a fresh dist/ and the files every app folder carries. With `remote`, a
 * credential that holds export_data, it adds the published app's data/ from
 * project_export. No secret and nothing of .konae/ goes with it.
 */
export async function exportLocal(root: string, output: string, remote?: { url: string; token: string }) {
  await assertEmptyFolder(output);
  const settings = await readSettings(root);
  await buildLocal(root);
  await mkdir(output, { recursive: true });
  await cp(appDir(root), path.join(output, SOURCES_FOLDER), { recursive: true, filter: (source) => !path.basename(source).startsWith(".") && path.basename(source) !== "node_modules" });
  await cp(path.join(root, DIST_FOLDER), path.join(output, DIST_FOLDER), { recursive: true });
  await writeFile(path.join(output, SETTINGS_FILE), `${JSON.stringify(settings, null, 2)}\n`);
  await writeEntries(output, await folderFiles(root, settings));
  if (!remote) return { output, data: false };
  const client = new McpClient(endpoint(remote.url), { token: remote.token });
  const published = await client.call<{ deployment?: { id: string } | null }>("project_inspect", { projectId: settings.id });
  if (!published.deployment) throw Error("Publish the app before exporting its data, or leave out --with-data.");
  await downloadData(client, published.deployment.id, output);
  return { output, data: true };
}

/**
 * Writes the Action Contract of the Capsule in `root` into `out`: the JSON,
 * its schema and the Markdown. The project's ID and name come from
 * konae.json when there is one.
 */
export async function writeContract(root: string, out = root) {
  const read = async (file: string) => readFile(path.join(appDir(root), file), "utf8");
  let project: { id: string; name: string } | undefined;
  try {
    const settings = await json<AppSettings>(path.join(root, SETTINGS_FILE));
    project = { id: settings.id, name: settings.name };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  let actionsSource: string;
  try { actionsSource = await read("actions.json"); } catch { throw Error(`No actions.json in ${appDir(root)}.`); }
  const schemaSource = await read("schema.sql").catch(() => "");
  const { contract, warnings } = generateActionContract({ actionsSource, schemaSource, project });
  await mkdir(out, { recursive: true });
  await writeFile(path.join(out, ACTION_CONTRACT_FILE), serializeActionContract(contract));
  await writeFile(path.join(out, ACTION_CONTRACT_SCHEMA_FILE), `${JSON.stringify(actionContractSchema, null, 2)}\n`);
  await writeFile(path.join(out, ACTION_CONTRACT_MARKDOWN), renderActionContractMarkdown(contract));
  await writeActionTypes(out, contract);
  return { contract, warnings, file: path.join(out, ACTION_CONTRACT_FILE) };
}

/** Writes actions.d.ts into `out` when its text changed, so a watcher that sees the file sees a real change. */
export async function writeActionTypes(out: string, contract: ActionContract) {
  const file = path.join(out, ACTION_TYPES_FILE);
  const text = renderActionTypes(contract);
  if (await readFile(file, "utf8").catch(() => undefined) !== text) await writeFile(file, text);
}

/**
 * Writes actions.d.ts beside konae.json from app/actions.json and
 * app/schema.sql, as dev does on each start and change. When they do not parse, the last one stays.
 */
async function refreshActionTypes(root: string) {
  const actionsSource = await readFile(path.join(appDir(root), "actions.json"), "utf8").catch(() => undefined);
  if (actionsSource === undefined) return;
  const schemaSource = await readFile(path.join(appDir(root), "schema.sql"), "utf8").catch(() => "");
  try {
    await writeActionTypes(root, generateActionContract({ actionsSource, schemaSource }).contract);
  } catch (error) {
    console.warn(`${ACTION_TYPES_FILE} stays as it was: ${error instanceof Error ? error.message : error}`);
  }
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
 * Writes a new app folder without a Kernel: the template in app/, konae.json
 * and the files every app folder carries. The app's ID is made here; the
 * first publish registers it on the installation with project_create, which
 * takes an ID the caller chose.
 */
export async function initLocal(root: string, options: { name?: string; template?: string } = {}) {
  // The file project_create answers from, so a folder starts the same through the CLI and through MCP.
  const all = await json<ProjectTemplates>(path.join(toolRoot, PROJECT_TEMPLATES_FILE));
  const born = all.templates[options.template ?? all.default];
  if (!born) throw Error(`Unknown template ${options.template}. Choose one of: ${INIT_TEMPLATES.join(", ")}.`);
  await assertEmptyFolder(root);
  const name = options.name ?? "My app";
  for (const [filename, source] of Object.entries(born.files)) {
    const target = path.resolve(appDir(root), filename);
    if (!target.startsWith(path.resolve(appDir(root)) + path.sep)) throw Error("Invalid template path.");
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, source, { flag: "wx" });
  }
  // The template says who may open the app. Only people the owner invites can create an account.
  const settings: AppSettings = {
    id: `app_${crypto.randomUUID()}`, name, slug: slugOf(name), visibility: born.visibility,
    signupPolicy: { mode: INIT_SIGNUP_POLICY.mode, allowlist: [] }, signInMethods: ["password"], kernel: KERNEL_VERSION, manifest: { version: 1 },
  };
  await writeFile(path.join(root, SETTINGS_FILE), `${JSON.stringify(settings, null, 2)}\n`, { flag: "wx" });
  await writeEntries(root, await folderFiles(root, settings));
  await ignoreInGit(root, gitignore.trim().split("\n"));
  return settings;
}

/** The Kernel `dev` runs on the folder, and the commands `sql`, `events` and `dev` give it. */
export type LocalKernel = { binary: string; env: Record<string, string | undefined>; data: string; run(args: string[], input?: string): Promise<string> };

/**
 * The `kernel` binary on this machine and the environment that points it at
 * the folder. Its data directory is `.konae/kernel`, where `apps/<id>` links
 * back to the folder, so the Kernel reads `konae.json`, `app/`, `dist/` and
 * `data/` where they are. KONAE_KERNEL_BIN names a built `kernel` binary to
 * run instead of compiling one.
 */
/** 1 TiB: no file or app folder on this machine reaches it, so a local Kernel has no file limit in practice. */
const LOCAL_FILE_LIMIT = String(1024 ** 4);

export async function localKernel(root: string, settings: AppSettings, options: { port?: string; confirmSchema?: string; build?: boolean } = {}): Promise<LocalKernel> {
  const binary = process.env.KONAE_KERNEL_BIN ? path.resolve(process.env.KONAE_KERNEL_BIN) : path.join(toolRoot, "kernel-rs/target/debug/kernel");
  if (!process.env.KONAE_KERNEL_BIN && options.build !== false) {
    const compilation = Bun.spawn(["cargo", "build", "--manifest-path", path.join(toolRoot, "kernel-rs/Cargo.toml"), "-p", "kernel"], { stdout: "inherit", stderr: "inherit" });
    if (await compilation.exited !== 0) throw Error("Could not build the Kernel. Install Rust, or set KONAE_KERNEL_BIN to a kernel binary built from this checkout.");
  }
  if (!existsSync(binary)) throw Error(`No kernel binary at ${binary}. Run dev once, or set KONAE_KERNEL_BIN.`);
  const data = path.join(root, ".konae/kernel");
  const link = path.join(data, "apps", settings.id);
  await mkdir(path.dirname(link), { recursive: true });
  if (await readlink(link).catch(() => undefined) === undefined) await symlink(root, link, "dir");
  await ignoreInGit(root, [".konae/"]);
  const port = options.port ?? "8796";
  const env: Record<string, string | undefined> = {
    ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KONAE_") && !name.startsWith("VITE_"))),
    KONAE_BIND: "127.0.0.1", KONAE_DATA_DIR: data, KONAE_KERNEL_PORT: port,
    KONAE_PUBLIC_ORIGIN: `http://localhost:${port}`, KONAE_HOST_SUFFIX: "localhost",
    // Account links and notices land in a file on this machine, never a provider.
    KONAE_MAIL_TRANSPORT: "development",
    // A local run takes files of any size unless these name a limit, so uploads can be tried with real files.
    KONAE_FILES_QUOTA_BYTES: process.env.KONAE_FILES_QUOTA_BYTES || LOCAL_FILE_LIMIT,
    KONAE_FILE_MAX_BYTES: process.env.KONAE_FILE_MAX_BYTES || LOCAL_FILE_LIMIT,
    // Shortens every schedule so a run can be watched; the Kernel takes it only on loopback.
    ...(process.env.KONAE_SCHEDULE_MIN_SECONDS ? { KONAE_SCHEDULE_MIN_SECONDS: process.env.KONAE_SCHEDULE_MIN_SECONDS } : {}),
    ...(options.confirmSchema ? { KONAE_CONFIRM_SCHEMA: options.confirmSchema } : {}),
  };
  const run = async (args: string[], input?: string) => {
    const child = Bun.spawn([binary, ...args, "--app", settings.id], { env, stdin: input === undefined ? "ignore" : new Blob([input]), stdout: "pipe", stderr: "pipe" });
    const [code, output, error] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
    if (code !== 0) throw Error(error.trim().replace(/^Error: /, "") || `kernel ${args[0]} stopped with ${code}.`);
    return output;
  };
  return { binary, env, data, run };
}

/**
 * Sets a local value for each signing secret actions.json names that the
 * local app does not hold yet, and returns one line for each secret it set.
 * The Kernel does not serve an app whose route signs with an unset secret,
 * and the published app's values never leave its installation.
 */
export async function setDevSecrets(root: string, kernel: Pick<LocalKernel, "run">) {
  let actions: Record<string, { http?: { signature?: { secret?: string } }; auth?: { secret?: string } }>;
  try { actions = JSON.parse(await readFile(path.join(appDir(root), "actions.json"), "utf8")).actions ?? {}; } catch { return []; }
  const wanted = new Map<string, "route" | "call">();
  for (const action of Object.values(actions)) {
    if (typeof action?.http?.signature?.secret === "string") wanted.set(action.http.signature.secret, "route");
    if (typeof action?.auth?.secret === "string") wanted.set(action.auth.secret, "call");
  }
  if (wanted.size === 0) return [];
  const held = new Set((JSON.parse(await kernel.run(["secret", "list"])) as { secrets?: { name: string }[] }).secrets?.map((secret) => secret.name));
  const lines: string[] = [];
  for (const [name, use] of wanted) {
    if (held.has(name)) continue;
    // whsec_ and base64, so a svix route takes it too.
    const value = `whsec_${Buffer.from(crypto.getRandomValues(new Uint8Array(24))).toString("base64")}`;
    await kernel.run(["secret", "set", name, "--use", use], value);
    lines.push(`Set the secret ${name} of the local app to ${value}. Sign local requests with it; the published app keeps its own.`);
  }
  return lines;
}

/** The local app's record of a range, from one run of the kernel binary's events command. */
export function kernelRecord(kernel: Pick<LocalKernel, "run">): ReadRecord {
  return async ({ from, to }) => JSON.parse(await kernel.run(["events", "--from", from, "--to", to]));
}

/**
 * Replaces data/ with the data `pull` exported into .konae/pull, through
 * the kernel binary's import, or empties it when there is none. `dev
 * --reset` runs it after the build and before the Kernel starts.
 */
export async function resetData(root: string, kernel: Pick<LocalKernel, "run">, pulled: Pulled | undefined, confirmSchema?: string) {
  if (!pulled) { await rm(path.join(root, DATA_FOLDER), { recursive: true, force: true }); return; }
  await kernel.run(["import", path.join(root, PULL_DIRECTORY), ...(confirmSchema ? ["--confirm-schema", confirmSchema] : [])]);
  await writeFile(path.join(root, IMPORTED_MARKER), JSON.stringify({ packageId: pulled.manifest.packageId }));
}

/** Which export of the published app's data `dev --reset` last put in data/. */
const IMPORTED_MARKER = ".konae/pull-imported.json";

/** init and export write a whole app folder, so the target must hold nothing but dotfiles (.git, .konae, .gitignore). */
async function assertEmptyFolder(root: string) {
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

/**
 * The arguments `tool` sends: a JSON object, or {} when none is given.
 * `piped` is the one argument `--stdin <key>` reads from standard input,
 * without its final line break, as `kernel secret set` reads a value.
 */
export function toolArguments(text?: string, piped?: { key: string; value: string }): Record<string, unknown> {
  let value: unknown = {};
  if (text !== undefined) {
    try { value = JSON.parse(text); } catch { value = undefined; }
    if (!value || typeof value !== "object" || Array.isArray(value)) throw Error(`The arguments must be one JSON object, such as '{"projectId":"app_1"}', not ${text}.`);
  }
  const arguments_ = value as Record<string, unknown>;
  if (piped) arguments_[piped.key] = piped.value.replace(/[\r\n]+$/, "");
  return arguments_;
}

async function main() {
  const argv = process.argv.slice(2);
  if (argv[0] === "--") argv.shift();
  const [command, ...args] = argv;
  if (!command || ["help", "--help"].includes(command)) {
    console.log(HELP);
    return;
  }
  if (!["login", "init", "build", "contract", "conform", "export", "publish", "dev", "pull", "sql", "events", "domain", "tool"].includes(command)) throw Error(`Unknown command ${command}.\n\n${HELP}`);
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
    const settings = await initLocal(root, { name: option("name"), template: option("template") });
    console.log(`Created ${settings.name} in ${root}. Run it with: bun run konae -- dev --dir ${root}`);
    return;
  }
  if (command === "build") { await buildLocal(root); console.log(`Built ${path.join(root, DIST_FOLDER)}`); return; }
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
  // The installation every remote command talks to: --url, then the one login saved beside the token it would send.
  const installation = async () => {
    const url = option("url") ?? (await json<{ url?: string }>(path.join(root, ".konae/credentials.json")).catch(() => undefined))?.url;
    if (!url) throw Error(NO_INSTALLATION);
    return url;
  };
  // The owner's MCP on that installation, with KONAE_MCP_TOKEN or the token login saved.
  const ownerClient = async () => new McpClient(endpoint(await installation()), { token: await credentials(root) });
  if (command === "domain") {
    const deploymentId = option("deployment");
    if (!deploymentId) throw Error("Name the Deployment with --deployment <id>. project_inspect shows its ID.");
    for (const line of await domainLocal(await ownerClient(), deploymentId, { set: option("set"), clear: args.includes("--clear") })) console.log(line);
    return;
  }
  if (command === "tool") {
    // The name and the JSON are the two words that are neither a flag nor a flag's value.
    const [name, text] = args.filter((arg, i) => !arg.startsWith("--") && !["--dir", "--url", "--stdin"].includes(args[i - 1]));
    if (!name) throw Error("Name the tool: tool <name> ['<json arguments>']. docs/mcp-tools.md lists the tools, and tool session_inspect shows what this credential may call.");
    const key = option("stdin");
    const piped = key === undefined ? undefined : { key, value: await Bun.stdin.text() };
    console.log(JSON.stringify(await (await ownerClient()).call(name, toolArguments(text, piped)), null, 2));
    return;
  }
  const config = configOf(await readSettings(root));
  if (command === "export") {
    const out = option("out");
    if (!out) throw Error("Name an empty folder for the export with --out <folder>.");
    const withData = args.includes("--with-data");
    const exported = await exportLocal(root, path.resolve(out), withData ? { url: await installation(), token: await credentials(root) } : undefined);
    console.log(`Exported the app folder to ${exported.output}${exported.data ? ", with the published app's data in data/" : ""}. Its secrets stay where they are set.`);
    return;
  }
  if (command === "publish") {
    const url = await installation();
    console.log((await publishLocal(root, { ...config, url }, await credentials(root), console.warn, confirmOnTerminal(option("confirm-schema")))).url);
    return;
  }
  // sql and events read the local app through the Kernel's commands, and the published Deployment only with --prod.
  const target = async (): Promise<{ deploymentId: string; feed?: string } & ({ local: LocalKernel; client?: undefined } | { client: McpClient; local?: undefined })> => {
    if (!args.includes("--prod")) {
      const settings = await readSettings(root);
      const local = await localKernel(root, settings, { build: false });
      const session = await json<{ events?: string }>(path.join(root, DEV_SESSION)).catch(() => undefined);
      return { deploymentId: settings.id, local, feed: session?.events };
    }
    const client = await ownerClient();
    const remote = await client.call<{ deployment?: { id: string } | null }>("project_inspect", { projectId: config.project.id });
    if (!remote.deployment) throw Error("This project has no Deployment yet. Publish it first.");
    return { client, deploymentId: remote.deployment.id };
  };
  if (command === "sql") {
    const file = option("file");
    // The statement is the one argument that is neither a flag nor a flag's value.
    const inline = args.find((arg, i) => !arg.startsWith("--") && !["--dir", "--url", "--file"].includes(args[i - 1]));
    const sql = file ? await readFile(path.resolve(file), "utf8") : inline;
    if (!sql?.trim()) throw Error("Give the statement in quotes or with --file <file.sql>.");
    const found = await target();
    const answer = found.local ? JSON.parse(await found.local.run(["sql", sql])) : await found.client.call("database_execute", { deploymentId: found.deploymentId, operation: "sql", input: { sql } });
    console.log(JSON.stringify(answer, null, 2));
    return;
  }
  if (command === "events") {
    const { deploymentRecord, formatEvent, readEvents } = await import("./events");
    const found = await target();
    const { deploymentId, local, feed } = found;
    const record = found.local ? kernelRecord(found.local) : deploymentRecord(found.client, deploymentId);
    const days = Number(option("days", "7"));
    if (!Number.isInteger(days) || days < 1 || days > 400) throw Error("--days takes a whole number from 1 to 400.");
    const asJson = args.includes("--json");
    const seen = new Map<number, number>();
    const printed = new Set<string>();
    const read = async () => {
      // Only the local Kernel has a mailbox this machine can read.
      // dev's events page stamps each mail when it is written, so its answer is preferred to a read from here.
      const served = feed ? await fetch(`${feed}/events.json?days=${days}`).then((response) => response.ok ? response.json() as Promise<AppEvent[]> : undefined).catch(() => undefined) : undefined;
      const events = served ?? await readEvents(record, deploymentId, { days, seen, mailbox: local ? path.join(local.data, "mailbox.jsonl") : undefined });
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
    console.log(`Exported the data ${describePulled(pulled)} to ${path.join(root, PULL_DIRECTORY)}. It holds the app's accounts and rows, so keep .konae out of Git.`);
    console.log(`Import it into the local app with: bun run konae -- dev --dir ${root} --reset`);
    return;
  }
  if (command === "dev") {
    const settings = await readSettings(root);
    const port = option("port", "8796")!;
    if (!/^\d+$/.test(port) || Number(port) < 1024 || Number(port) > 65535) throw Error("Choose a local port between 1024 and 65535.");
    const kernel = await localKernel(root, settings, { port, confirmSchema: option("confirm-schema") });
    const pulled = await readPulled(root);
    let reset = args.includes("--reset");
    if (!reset && pulled && (await json<{ packageId?: string }>(path.join(root, IMPORTED_MARKER)).catch(() => undefined))?.packageId !== pulled.manifest.packageId) {
      console.log(`Exported data waits in ${path.join(root, PULL_DIRECTORY)}. Run dev with --reset to replace data/ with it.`);
    }
    const devSession = path.join(root, DEV_SESSION);
    let child: ReturnType<typeof Bun.spawn> | undefined;
    let feed: { url: string; stop(): void } | undefined;
    // Builds dist/, sets the local secrets, imports the pulled data on the first start with --reset, and starts the Kernel, which opens the folder as a release.
    const start = async () => {
      for (const line of await setDevSecrets(root, kernel)) console.log(line);
      await refreshActionTypes(root);
      await buildLocal(root);
      if (reset) {
        reset = false;
        await resetData(root, kernel, pulled, option("confirm-schema"));
        console.log(pulled
          ? `Replaced data/ with the data ${describePulled(pulled)}, after a backup in ${path.join(root, ".konae/backups")}. Its accounts keep their passwords; to sign in as one without knowing it, ask the app for a password reset and open the link in ${path.join(kernel.data, "mailbox.jsonl")}.`
          : "Emptied data/. The app starts with no rows and no accounts.");
      }
      const started = Bun.spawn([kernel.binary], { env: kernel.env, stdout: "pipe", stderr: "pipe" });
      child = started;
      let output = "";
      void (async () => {
        const decoder = new TextDecoder();
        for await (const block of started.stdout as ReadableStream<Uint8Array>) output += decoder.decode(block);
      })();
      void (async () => {
        const decoder = new TextDecoder();
        for await (const block of started.stderr as ReadableStream<Uint8Array>) { const text = decoder.decode(block); output += text; process.stderr.write(text); }
      })();
      for (let attempt = 0; attempt < 150 && !/listening on/.test(output); attempt++) {
        if (started.exitCode !== null) break;
        await Bun.sleep(100);
      }
      if (!/listening on/.test(output)) throw Error(`The Kernel stopped before it served the app.\n${output.trim()}`);
      const served = /Serving .+ at (\S+)/.exec(output)?.[1];
      if (!served) throw Error("The Kernel does not serve this folder. It said why above.");
      return served;
    };
    const stop = async () => { if (child && child.exitCode === null) { child.kill(); await child.exited; } };
    process.once("SIGINT", () => { void stop().then(() => process.exit(130)); });
    process.once("SIGTERM", () => { void stop().then(() => process.exit(143)); });
    process.once("exit", () => { child?.kill(); });
    try {
      console.log(await start());
      // The events page answers on the next port, on this machine only. A taken port leaves the command, which reads the Kernel itself.
      const { serveEvents } = await import("./events");
      const eventsPort = Number(option("events-port", String(Number(port) + 1)));
      try { feed = serveEvents(kernelRecord(kernel), settings.id, path.join(kernel.data, "mailbox.jsonl"), eventsPort); }
      catch { console.warn(`Port ${eventsPort} is taken, so the events page is off. Choose another with --events-port, or run events.`); }
      if (feed) await writeFile(devSession, JSON.stringify({ events: feed.url }), { mode: 0o600 });
      const policy = settings.signupPolicy.mode;
      if (policy !== "open") {
        // The Kernel's own settings, so a command run by hand seals and addresses its links as the Kernel does.
        const envFile = path.join(root, ".konae/kernel.env");
        await writeFile(envFile, Object.entries(kernel.env).filter(([name]) => name.startsWith("KONAE_")).map(([name, value]) => `${name}=${value}\n`).join(""), { mode: 0o600 });
        console.log(`The Signup Policy is ${policy}. To create a test account, invite it, and open the link that lands in ${path.join(kernel.data, "mailbox.jsonl")}:`);
        console.log(`  env $(cat ${envFile}) ${kernel.binary} invite <email> --app ${settings.id}`);
      }
      console.log(feed
        ? `Email, calls, scheduled runs and Signals of this app: ${feed.url} in a browser, or bun run konae -- events --dir ${root} --json`
        : `See the email, calls, scheduled runs and Signals of this app with: bun run konae -- events --dir ${root} --follow`);
      let working = false; let queued = false;
      const restart = async () => {
        if (working) { queued = true; return; } working = true;
        do { queued = false; try { await stop(); console.log(`Updated ${await start()}. Reload the app to see changes.`); } catch (e) { console.error(e instanceof Error ? e.message : e); } } while (queued);
        working = false;
      };
      let timer: ReturnType<typeof setTimeout>;
      const watcher = watch(appDir(root), { recursive: true }, (_, filename) => {
        if (!filename || filename.startsWith(".") || filename.includes("node_modules") || !/\.(?:html|css|js|jsx|ts|tsx|json|sql)$/.test(filename)) return;
        clearTimeout(timer); timer = setTimeout(() => void restart(), 200);
      });
      await new Promise(() => {});
      watcher.close();
    } finally { await stop(); feed?.stop(); await rm(devSession, { force: true }); }
    return;
  }
}

if (import.meta.main) main().catch(error => { console.error(error.message); process.exitCode = 1; });
