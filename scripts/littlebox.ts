import { chmod, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { watch } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { McpClient, type Transfer } from "../src/mcp/client";
import { normalizeManifest } from "../src/deployments/manifest";
import { checkPackageManifest, packageComponentFiles, type PackageManifest } from "../src/lib/portable-package";
import { downloadToFile, writeTarFile, type FileTarEntry } from "./package-files";
import {
  ACTION_CONTRACT_FILE, ACTION_CONTRACT_MARKDOWN, ACTION_CONTRACT_SCHEMA_FILE,
  actionContractSchema, generateActionContract, renderActionContractMarkdown, serializeActionContract,
} from "../src/lib/action-contract";
import { checkEjectPackage, type EjectPlan, type EjectReceipt } from "../src/lib/eject-package";
import { assembleStaticArtifact } from "../src/lib/static-artifact.js";

const toolRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
type Project = { id: string; name: string; slug: string; manifest: Record<string, unknown>; files?: Record<string, string> };
type Config = { url: string; project: Project };
type SignupMode = "open" | "invitation" | "closed" | "allowlist";
export type SignupPolicy = { mode: SignupMode; allowlist: string[] };

const SIGNUP_MODES: readonly SignupMode[] = ["open", "invitation", "closed", "allowlist"];
const MODE_LABELS: Record<SignupMode, string> = { open: "Open", invitation: "Invitation only", closed: "Closed", allowlist: "Allowlist" };
/** What `init` writes: a private app that admits the people its owner invites. */
export const INIT_SIGNUP_POLICY = { mode: "invitation" } as const;

/**
 * `project.manifest.signupPolicy` in littlebox.json: who may create an app
 * account when the first release goes out. Checked before anything is sent;
 * the Kernel checks the allowlist entries again.
 */
export function manifestSignupPolicy(manifest: Record<string, unknown>): SignupPolicy | undefined {
  const value = manifest.signupPolicy as { mode?: unknown; allowlist?: unknown } | undefined;
  if (value === undefined) return undefined;
  if (!value || typeof value !== "object" || !SIGNUP_MODES.includes(value.mode as SignupMode)) {
    throw Error('littlebox.json project.manifest.signupPolicy.mode must be "open", "invitation", "closed" or "allowlist".');
  }
  const allowlist = value.allowlist ?? [];
  if (!Array.isArray(allowlist) || !allowlist.every((entry) => typeof entry === "string")) {
    throw Error("littlebox.json project.manifest.signupPolicy.allowlist must be a list of emails and @domains.");
  }
  return { mode: value.mode as SignupMode, allowlist };
}

function describePolicy(policy: SignupPolicy) {
  return policy.mode === "allowlist" ? `Allowlist (${policy.allowlist.join(", ") || "empty"})` : MODE_LABELS[policy.mode];
}

/**
 * The warning a republish prints when littlebox.json names another policy
 * than the Deployment has. A publish never changes who may join an app that
 * exists (docs/mcp.md, "Signup policy"), so the file and the app can differ.
 */
export function signupPolicyWarning(wanted: SignupPolicy, current: SignupPolicy): string | undefined {
  const entries = (policy: SignupPolicy) => policy.allowlist.map((entry) => entry.trim().toLowerCase()).filter(Boolean).sort().join("\n");
  if (wanted.mode === current.mode && (wanted.mode !== "allowlist" || entries(wanted) === entries(current))) return undefined;
  return `littlebox.json asks for signup ${describePolicy(wanted)}, but this app has ${describePolicy(current)}. A publish sets the policy only on the first release and never changes who may join afterwards. Change it in the editor's Auth > Users view, or call signup_policy_set with a credential that holds manage_users.`;
}

function endpoint(url: string) { return new URL("/mcp", url).href; }
async function json<T>(filename: string): Promise<T> { return JSON.parse(await readFile(filename, "utf8")); }

/** Signs in, or creates the account with an invitation code or, for the first owner, the setup key. */
async function login(url: string, email: string, password: string, admission: { code?: string; setupKey?: string } = {}, scopes = ["read", "write", "publish"]): Promise<string> {
  let response = await fetch(new URL("/_littlebox/owner/login", url), {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, password }),
  });
  if (!response.ok && (admission.code || admission.setupKey)) response = await fetch(new URL("/_littlebox/owner/signup", url), {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, password, ...admission }),
  });
  if (response.status === 202) throw Error("This installation verifies email. Open the link sent to your address to choose your password, then run login again.");
  if (!response.ok) throw Error((await response.json()).error ?? "Owner sign-in failed.");
  const cookie = response.headers.get("set-cookie")?.split(";")[0];
  if (!cookie) throw Error("Owner sign-in returned no session.");
  const client = new McpClient(endpoint(url), { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  const token = await client.call<{ token: string }>("token_create", { scopes, ttlSeconds: 86400 });
  await fetch(new URL("/_littlebox/owner/logout", url), { method: "POST", headers: { cookie } });
  return token.token;
}

async function credentials(root: string) {
  if (process.env.LITTLEBOX_MCP_TOKEN) return process.env.LITTLEBOX_MCP_TOKEN;
  return (await json<{ token: string }>(path.join(root, ".littlebox/credentials.json"))).token;
}

/** Files the CLI writes beside the Capsule, which are never uploaded as its source. */
const generatedFiles = new Set(["littlebox.json", ACTION_CONTRACT_FILE, ACTION_CONTRACT_SCHEMA_FILE]);

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
  const result = await build({ entryPoints: [path.join(root, entry)], bundle: true, write: false, platform: "browser", format: "esm", target: "es2022", outdir: "out" });
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
  const config = await json<Partial<Config>>(path.join(root, "littlebox.json")).catch(() => undefined);
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
  });
  await mkdir(path.join(root, ".littlebox"), { recursive: true });
  await writeFile(path.join(root, ".littlebox/artifact.json"), JSON.stringify(artifact));
  return { artifact, files };
}

/**
 * Builds and publishes. On a first release `project.manifest.signupPolicy`
 * goes as `initialSignupPolicy`; on a later one the Kernel keeps the app's
 * policy and `warn` gets one line when littlebox.json names another.
 */
export async function publishLocal(root: string, config: Config, token: string, warn: (message: string) => void = console.warn) {
  const client = new McpClient(endpoint(config.url), { token });
  const wanted = manifestSignupPolicy(config.project.manifest);
  // The policy is not part of the Capsule manifest, so the Kernel's project record never holds an allowlist.
  const { signupPolicy: _, ...manifest } = config.project.manifest;
  const project = { ...config.project, manifest };
  await client.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest });
  const remote = await client.call<{ deployment?: { id: string } | null }>("project_inspect", { projectId: project.id });
  const first = !remote.deployment;
  const { artifact, files } = await buildLocal(root);
  const payload = {
    project: { ...project, files }, visibility: manifest.visibility ?? "private", artifact,
    ...(first && wanted ? { initialSignupPolicy: wanted } : {}),
  };
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify(payload)), project.id);
  let receipt: { id: string; url: string };
  try {
    const plan = await client.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    await writeFile(path.join(root, ".littlebox/release-plan.json"), JSON.stringify(plan, null, 2));
    receipt = await client.call<{ id: string; url: string }>("release_apply", { planId: plan.planId });
    await writeFile(path.join(root, ".littlebox/deployment.json"), JSON.stringify(receipt, null, 2));
  } finally {
    try { await client.cancel(transfer.transferId); } catch (error) { console.warn(`Could not release the upload: ${error instanceof Error ? error.message : error}`); }
  }
  if (!first && wanted) {
    // Reading the policy needs only read. A Kernel that cannot answer leaves the publish as it is.
    const current = await client.call<{ mode: SignupMode; allowlist?: string[] }>("signup_policy_read", { deploymentId: receipt.id }).catch(() => undefined);
    const warning = current?.mode && signupPolicyWarning(wanted, { mode: current.mode, allowlist: current.allowlist ?? [] });
    if (warning) warn(warning);
  }
  return receipt;
}

export async function exportLocal(root: string, config: Config, output: string, token?: string) {
  const { artifact, files } = await buildLocal(root);
  let manifest = config.project.manifest;
  let captured: { package: PackageManifest; files: FileTarEntry[] } | undefined;
  const scratch = token === undefined ? undefined : await mkdtemp(path.join(tmpdir(), "littlebox-package-"));
  try {
    if (token !== undefined && scratch) {
      const client = new McpClient(endpoint(config.url), { token });
      const remote = await client.call<{ deployment?: { id: string; manifest?: Record<string, unknown> } }>("project_inspect", { projectId: config.project.id });
      if (!remote.deployment) throw Error("Publish the project before exporting its live data, or omit --with-data.");
      const deployedManifest = remote.deployment.manifest;
      if (deployedManifest?.visibility !== "public" && deployedManifest?.visibility !== "private") {
        throw Error("The deployment did not provide its visibility. Live data export stopped.");
      }
      manifest = deployedManifest;
      let exported: { manifest: unknown; downloads: Record<string, Transfer> };
      try {
        exported = await client.call("package_export", { deploymentId: remote.deployment.id });
      } catch (error) {
        if (error instanceof Error && error.message.includes("Unknown tool")) {
          throw Error("Exporting live data needs a credential with the export_data permission. Run login with --export-data, or create a token with export_data for this project.");
        }
        throw error;
      }
      const components: FileTarEntry[] = [];
      let checked: PackageManifest;
      try {
        checked = checkPackageManifest(exported.manifest);
        for (const content of checked.contents) {
          const transfer = exported.downloads[content.name];
          if (!transfer || transfer.size !== content.size || transfer.sha256 !== content.sha256) throw Error(`The ${content.name} download does not match the package manifest.`);
          const file = path.join(scratch, packageComponentFiles[content.name]);
          await downloadToFile(client, transfer, file);
          components.push({ path: `littlebox/package/${packageComponentFiles[content.name]}`, file, size: content.size });
        }
      } finally {
        for (const transfer of Object.values(exported.downloads)) await client.cancel(transfer.transferId).catch(() => {});
      }
      captured = { package: checked, files: components };
    }
    const [{ projectExportEntries }, { kernelFiles }] = await Promise.all([
      import("../src/lib/export-project-kit"), import("../src/lib/export-project-sources"),
    ]);
    const model = files["schema.ts"] ?? "";
    const project = {
      ...config.project,
      manifest: normalizeManifest({ ...manifest, visibility: manifest.visibility ?? "private" }),
      files, directories: [], updatedAt: Date.now(),
      databaseModel: { source: model, acceptedSource: model, revision: "external" },
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
 * littlebox.json when there is one.
 */
export async function writeContract(root: string, out = root) {
  const read = async (file: string) => readFile(path.join(root, file), "utf8");
  let project: { id: string; name: string } | undefined;
  try {
    const config = await json<Config>(path.join(root, "littlebox.json"));
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

/**
 * Eject: plan on the server at the saved-sources revision, download and check
 * the code package, write it into `root`, then apply. A stale revision is
 * refused by eject_plan, before anything downloads. The project changes only
 * after every file is on disk, and a run after an interruption finishes the
 * same Eject: eject_plan returns the applied plan and its receipt.
 */
export async function ejectLocal(root: string, url: string, projectId: string, token: string, expectedRevision?: number): Promise<EjectReceipt> {
  const marker = path.join(root, ".littlebox/eject-plan.json");
  let resuming = false;
  try { resuming = (await json<{ projectId?: string }>(marker)).projectId === projectId; } catch {}
  if (!resuming) await assertEmptyForInit(root);
  const client = new McpClient(endpoint(url), { token });
  const revision = expectedRevision ?? (await client.call<{ sourcesRevision: number }>("project_inspect", { projectId })).sourcesRevision;
  let plan: EjectPlan;
  try {
    plan = await client.call<EjectPlan>("eject_plan", { projectId, expectedRevision: revision });
  } catch (error) {
    if (error instanceof Error && error.message.includes("Unknown tool")) {
      throw Error("Eject needs a credential with the manage_lifecycle permission. Run login with --manage-lifecycle, or create a token with manage_lifecycle for this project.");
    }
    throw error;
  }
  const checked = await checkEjectPackage(await client.download(plan.transfer), plan);
  const targets = Object.entries(checked.files).map(([name, text]) => {
    const target = path.resolve(root, name);
    if (!target.startsWith(root + path.sep)) throw Error(`Invalid package path: ${name}`);
    return [target, text] as const;
  });
  // A resumed run keeps files it already wrote, but only if they are unchanged.
  for (const [target, text] of targets) {
    const existing = await readFile(target, "utf8").catch(() => undefined);
    if (existing !== undefined && existing !== text) throw Error(`${path.relative(root, target)} differs from the code package. Choose an empty directory.`);
  }
  await mkdir(path.join(root, ".littlebox"), { recursive: true });
  await writeFile(marker, JSON.stringify({ projectId, planId: plan.planId, sourcesRevision: plan.expectedRevision, sha256: plan.package.sha256 }, null, 2));
  for (const directory of checked.directories) await mkdir(path.resolve(root, directory), { recursive: true });
  for (const [target, text] of targets) {
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, text);
  }
  await ignoreInGit(root, [".littlebox/", "node_modules/"]);
  const receipt = plan.applied && plan.receipt ? plan.receipt : await client.call<EjectReceipt>("eject_apply", { planId: plan.planId });
  await writeFile(path.join(root, ".littlebox/eject-receipt.json"), JSON.stringify(receipt, null, 2));
  return receipt;
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

/** init writes a template into the directory, so it must hold nothing but dotfiles (.git, .littlebox, .gitignore). */
async function assertEmptyForInit(root: string) {
  let entries: string[] = [];
  try { entries = await readdir(root); } catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
  const occupied = entries.filter((name) => !name.startsWith("."));
  if (occupied.length > 0) throw Error(`Refusing to overwrite ${occupied.slice(0, 3).join(", ")}. Choose an empty directory.`);
}

async function main() {
  const argv = process.argv.slice(2);
  if (argv[0] === "--") argv.shift();
  const [command, ...args] = argv;
  if (!command || ["help", "--help"].includes(command)) {
    console.log("Use login, init, build, dev, publish, export, eject, contract, or conform. Options include --dir, --url, --name, --port, and --out. Export --with-data includes live data, accounts, and files; it needs a credential from login --export-data or a token with export_data. contract writes action-contract.json, its schema and action-contract.md from actions.json and schema.sql. conform --url <app URL> --contract <file> calls every action on a Kernel and checks the answers against the contract; it runs each mutation once, so point it at a test Deployment. Add --token <app session token> to call authenticated actions, --key <API key> for routes, --secret <name>=<value> to sign a route, and --inputs <file> with input values by action where a generated string will not do.");
    return;
  }
  const option = (name: string, fallback?: string) => {
    const i = args.indexOf(`--${name}`);
    if (i < 0) return fallback;
    const value = args[i + 1];
    if (value === undefined || value.startsWith("--")) throw Error(`--${name} needs a value.`);
    return value;
  };
  const root = path.resolve(option("dir", process.cwd())!);
  if (command === "login") {
    const url = option("url", process.env.LITTLEBOX_URL ?? "http://localhost:4173")!;
    const email = process.env.LITTLEBOX_EMAIL; const password = process.env.LITTLEBOX_PASSWORD;
    if (!email || !password) throw Error("Set LITTLEBOX_EMAIL and LITTLEBOX_PASSWORD. To create the account, also set LITTLEBOX_INVITATION_CODE, or LITTLEBOX_SETUP_KEY for the first owner.");
    const scopes = ["read", "write", "publish", ...(args.includes("--export-data") ? ["export_data"] : []), ...(args.includes("--manage-lifecycle") ? ["manage_lifecycle"] : [])];
    const token = await login(url, email, password, { code: process.env.LITTLEBOX_INVITATION_CODE, setupKey: process.env.LITTLEBOX_SETUP_KEY }, scopes);
    await mkdir(path.join(root, ".littlebox"), { recursive: true });
    await writeFile(path.join(root, ".littlebox/credentials.json"), JSON.stringify({ url, token }), { mode: 0o600 });
    await chmod(path.join(root, ".littlebox/credentials.json"), 0o600);
    await ignoreInGit(root, [".littlebox/"]);
    console.log("Saved a 24-hour MCP credential in .littlebox/credentials.json. Keep .littlebox out of Git.");
    return;
  }
  if (command === "init") {
    const url = option("url", process.env.LITTLEBOX_URL ?? "http://localhost:4173")!;
    const client = new McpClient(endpoint(url), { token: await credentials(root) });
    // Checked before project_create: the template's file names are only known after it, and a refusal then would leave an orphan project.
    await assertEmptyForInit(root);
    const project = await client.call<Project>("project_create", { name: option("name", "My app")! });
    const files = project.files ?? {};
    for (const [filename, source] of Object.entries(files)) {
      const target = path.resolve(root, filename);
      if (!target.startsWith(root + path.sep)) throw Error("Invalid template path.");
      await mkdir(path.dirname(target), { recursive: true }); await writeFile(target, source, { flag: "wx" });
    }
    const { files: _, ...metadata } = project;
    // A new app starts private, and only people the owner invites can create an account.
    const manifest = { ...metadata.manifest, visibility: "private", signupPolicy: INIT_SIGNUP_POLICY };
    await writeFile(path.join(root, "littlebox.json"), JSON.stringify({ url, project: { ...metadata, manifest } }, null, 2), { flag: "wx" });
    await ignoreInGit(root, [".littlebox/", "node_modules/"]);
    console.log(`Created ${project.name} in ${root}.`);
    return;
  }
  if (command === "build") { await buildLocal(root); console.log("Built .littlebox/artifact.json"); return; }
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
  if (command === "eject") {
    const projectId = option("project");
    if (!projectId) throw Error("Name the project to eject with --project <id>.");
    const pinned = option("revision");
    if (pinned !== undefined && !/^\d+$/.test(pinned)) throw Error("--revision needs a whole number.");
    const url = option("url", process.env.LITTLEBOX_URL ?? "http://localhost:4173")!;
    const token = process.env.LITTLEBOX_MCP_TOKEN ?? (await json<{ token: string }>(path.join(root, ".littlebox/credentials.json"))).token;
    const receipt = await ejectLocal(root, url, projectId, token, pinned === undefined ? undefined : Number(pinned));
    console.log(`Ejected ${projectId} at sources revision ${receipt.sourcesRevision} into ${root}.${receipt.deployment ? ` ${receipt.deployment.url} keeps serving.` : ""}`);
    console.log(`Publish from this folder with: bun run littlebox -- publish --dir ${root}`);
    return;
  }
  const config = await json<Config>(path.join(root, "littlebox.json"));
  if (command === "export") {
    const output = path.resolve(option("out", path.join(root, ".littlebox/selfhost.tar"))!);
    await exportLocal(root, config, output, args.includes("--with-data") ? await credentials(root) : undefined);
    console.log(`Exported self-hosted app to ${output}.`);
    return;
  }
  if (command === "publish") { console.log((await publishLocal(root, config, await credentials(root))).url); return; }
  if (command === "dev") {
    await mkdir(path.join(root, ".littlebox/dev-data"), { recursive: true });
    const secretFile = path.join(root, ".littlebox/dev-secret");
    let secret: string;
    try { secret = await readFile(secretFile, "utf8"); } catch { secret = crypto.randomUUID() + crypto.randomUUID(); await writeFile(secretFile, secret, { mode: 0o600 }); }
    const port = option("port", "8796")!;
    if (!/^\d+$/.test(port) || Number(port) < 1024 || Number(port) > 65535) throw Error("Choose a local port between 1024 and 65535.");
    const compilation = Bun.spawn(["cargo", "build", "--manifest-path", path.join(toolRoot, "kernel-rs/Cargo.toml")], { stdout: "inherit", stderr: "inherit" });
    if (await compilation.exited !== 0) throw Error("Could not build the Development Kernel.");
    const env: Record<string, string | undefined> = { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_") && !key.startsWith("VITE_"))), LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_DATA_DIR: path.join(root, ".littlebox/dev-data"), LITTLEBOX_ADMIN_KEY: secret, LITTLEBOX_KERNEL_PORT: port, LITTLEBOX_PUBLIC_ORIGIN: `http://localhost:${port}`, LITTLEBOX_HOST_SUFFIX: "localhost", LITTLEBOX_BUN_PATH: process.execPath, LITTLEBOX_SOURCE_ROOT: toolRoot };
    for (const key of Object.keys(env)) if (key.startsWith("LITTLEBOX_S3_") || ["LITTLEBOX_ARTIFACT_BUCKET", "LITTLEBOX_DATA_BUCKET", "LITTLEBOX_MCP_ORIGIN"].includes(key)) delete env[key];
    const child = Bun.spawn([path.join(toolRoot, "kernel-rs/target/debug/littlebox-kernel")], { env, stdout: "inherit", stderr: "inherit" });
    const stop = () => { child.kill(); };
    process.once("SIGINT", stop); process.once("SIGTERM", stop);
    process.once("exit", stop);
    const url = `http://127.0.0.1:${port}`;
    let ready = false;
    for (let attempt = 0; attempt < 100; attempt++) {
      if (child.exitCode !== null) throw Error("Development Kernel stopped before becoming ready.");
      try { ready = (await fetch(`${url}/_littlebox/health`)).ok; } catch {}
      if (ready) break; await Bun.sleep(100);
    }
    if (!ready) { child.kill(); throw Error("Development Kernel did not become ready."); }
    // The development Kernel opens signup below, which needs manage_users.
    const token = await login(url, "local-owner@littlebox.test", secret, { setupKey: secret }, ["read", "write", "publish", "manage_users"]);
    // Development accounts come from signup, so the local app starts Open and a republish never warns.
    const devConfig = { ...config, url, project: { ...config.project, manifest: { ...config.project.manifest, signupPolicy: { mode: "open" } } } };
    try {
      const first = await publishLocal(root, devConfig, token);
      // dev-data kept from an earlier run may hold another policy. The development
      // Kernel is isolated and local, so let the developer create test accounts.
      await new McpClient(endpoint(url), { token }).call("signup_policy_set", { deploymentId: first.id, mode: "open" });
      console.log(first.url);
      let working = false; let queued = false;
      const rebuild = async () => {
        if (working) { queued = true; return; } working = true;
        do { queued = false; try { console.log(`Updated ${(await publishLocal(root, devConfig, token)).url}. Reload the app to see changes.`); } catch (e) { console.error(e instanceof Error ? e.message : e); } } while (queued);
        working = false;
      };
      let timer: ReturnType<typeof setTimeout>;
      const watcher = watch(root, { recursive: true }, (_, filename) => {
        if (!filename || filename.startsWith(".") || filename.includes("node_modules") || !/\.(?:html|css|js|jsx|ts|tsx|json|sql)$/.test(filename)) return;
        clearTimeout(timer); timer = setTimeout(() => void rebuild(), 200);
      });
      await child.exited; watcher.close();
    } finally { child.kill(); }
    return;
  }
  throw Error("Use login, init, build, dev, publish, export, eject, contract, or conform. Options include --dir, --url, --name, --port, and --out.");
}

if (import.meta.main) main().catch(error => { console.error(error.message); process.exitCode = 1; });
