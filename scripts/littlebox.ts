import { chmod, mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { watch } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { decodeBase64, McpClient, type Transfer } from "../src/mcp/client";
import { normalizeManifest } from "../src/deployments/manifest";
import type { PortableProjectState } from "../src/deployments/client";

const toolRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
type Project = { id: string; name: string; slug: string; manifest: Record<string, unknown>; files?: Record<string, string> };
type Config = { url: string; project: Project };

function endpoint(url: string) { return new URL("/mcp", url).href; }
async function json<T>(filename: string): Promise<T> { return JSON.parse(await readFile(filename, "utf8")); }

async function login(url: string, email: string, password: string, setupKey?: string): Promise<string> {
  let response = await fetch(new URL("/_littlebox/owner/login", url), {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, password }),
  });
  if (!response.ok && setupKey) response = await fetch(new URL("/_littlebox/owner/signup", url), {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, password, setupKey }),
  });
  if (!response.ok) throw Error((await response.json()).error ?? "Owner sign-in failed.");
  const cookie = response.headers.get("set-cookie")?.split(";")[0];
  if (!cookie) throw Error("Owner sign-in returned no session.");
  const client = new McpClient(endpoint(url), { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  const token = await client.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish"], ttlSeconds: 86400 });
  await fetch(new URL("/_littlebox/owner/logout", url), { method: "POST", headers: { cookie } });
  return token.token;
}

async function credentials(root: string) {
  if (process.env.LITTLEBOX_MCP_TOKEN) return process.env.LITTLEBOX_MCP_TOKEN;
  return (await json<{ token: string }>(path.join(root, ".littlebox/credentials.json"))).token;
}

async function sources(root: string, prefix = ""): Promise<Record<string, string>> {
  const result: Record<string, string> = {};
  for (const entry of await readdir(path.join(root, prefix), { withFileTypes: true })) {
    if (entry.name.startsWith(".") || ["node_modules", "dist", "data", "target"].includes(entry.name)) continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) Object.assign(result, await sources(root, relative));
    else if (entry.isFile() && /\.(?:html|css|js|jsx|ts|tsx|json|sql)$/.test(entry.name) && entry.name !== "littlebox.json") result[relative] = await readFile(path.join(root, relative), "utf8");
  }
  return result;
}

export async function buildLocal(root: string) {
  const files = await sources(root);
  const entry = files["src/main.ts"] !== undefined ? "src/main.ts" : "app.js";
  const result = await build({ entryPoints: [path.join(root, entry)], bundle: true, write: false, platform: "browser", format: "esm", target: "es2022", outdir: "out" });
  const script = result.outputFiles.find((file) => file.path.endsWith(".js"))?.text;
  if (!script) throw Error("The local builder produced no JavaScript.");
  const styles = [files["styles.css"] ?? "", result.outputFiles.find((file) => file.path.endsWith(".css"))?.text ?? ""].join("\n");
  const sdk = await readFile(path.join(toolRoot, "src/mcp/runtime.js"), "utf8");
  const safe = (text: string) => text.replaceAll("</script", "<\\/script");
  const html = `<!doctype html><html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${styles}</style><body>${files["index.html"] ?? '<main id="app"></main>'}<script data-littlebox-runtime>${safe(sdk)}</script><script type="module">${safe(script)}</script></body></html>`;
  const artifact = { version: 1, entrypoint: "index.html", files: { "index.html": html } };
  await mkdir(path.join(root, ".littlebox"), { recursive: true });
  await writeFile(path.join(root, ".littlebox/artifact.json"), JSON.stringify(artifact));
  return { artifact, files };
}

export async function publishLocal(root: string, config: Config, token: string) {
  const client = new McpClient(endpoint(config.url), { token });
  await client.call("project_create", { projectId: config.project.id, name: config.project.name, slug: config.project.slug, manifest: config.project.manifest });
  const { artifact, files } = await buildLocal(root);
  const payload = { project: { ...config.project, files }, visibility: config.project.manifest.visibility ?? "private", artifact };
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify(payload)), config.project.id);
  try {
    const plan = await client.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    await writeFile(path.join(root, ".littlebox/release-plan.json"), JSON.stringify(plan, null, 2));
    const receipt = await client.call<{ id: string; url: string }>("release_apply", { planId: plan.planId });
    await writeFile(path.join(root, ".littlebox/deployment.json"), JSON.stringify(receipt, null, 2));
    return receipt;
  } finally {
    try { await client.cancel(transfer.transferId); } catch (error) { console.warn(`Could not release the upload: ${error instanceof Error ? error.message : error}`); }
  }
}

export async function exportLocal(root: string, config: Config, output: string, token?: string) {
  const { artifact, files } = await buildLocal(root);
  let state: PortableProjectState | undefined;
  let manifest = config.project.manifest;
  if (token !== undefined) {
    const client = new McpClient(endpoint(config.url), { token });
    const remote = await client.call<{ deployment?: { id: string; manifest?: Record<string, unknown> } }>("project_inspect", { projectId: config.project.id });
    if (!remote.deployment) throw Error("Publish the project before exporting its live data, or omit --with-data.");
    const deployedManifest = remote.deployment.manifest;
    if (deployedManifest?.visibility !== "public" && deployedManifest?.visibility !== "private") {
      throw Error("The deployment did not provide its visibility. Live data export stopped.");
    }
    manifest = deployedManifest;
    const transfer = await client.call<Transfer>("project_export", { deploymentId: remote.deployment.id });
    const body = JSON.parse(new TextDecoder().decode(await client.download(transfer))) as { result?: { initialDatabase: string; initialIdentity?: string; initialFiles?: string; initialSignupPolicy?: PortableProjectState["initialSignupPolicy"]; visibility?: unknown } };
    if (!body.result?.initialDatabase) throw Error("Portable export is missing its database.");
    const visibility = body.result.visibility;
    if (visibility !== undefined && visibility !== "public" && visibility !== "private") {
      throw Error("The portable snapshot has invalid visibility. Live data export stopped.");
    }
    state = {
      initialDatabase: decodeBase64(body.result.initialDatabase),
      ...(body.result.initialIdentity ? { initialIdentity: decodeBase64(body.result.initialIdentity) } : {}),
      ...(body.result.initialFiles ? { initialFiles: decodeBase64(body.result.initialFiles) } : {}),
      ...(body.result.initialSignupPolicy ? { initialSignupPolicy: body.result.initialSignupPolicy } : {}),
      ...(visibility !== undefined ? { visibility } : {}),
    };
  }
  const [{ buildProjectExport }, { kernelFiles }] = await Promise.all([
    import("../src/lib/export-project-kit"), import("../src/lib/export-project-sources"),
  ]);
  const model = files["schema.ts"] ?? "";
  const project = {
    ...config.project,
    manifest: normalizeManifest({ ...manifest, visibility: manifest.visibility ?? "private" }),
    files, directories: [], updatedAt: Date.now(),
    databaseModel: { source: model, acceptedSource: model, revision: "external" },
  };
  const { archive } = buildProjectExport(project, artifact, kernelFiles, state);
  await writeFile(output, archive, { flag: "wx", mode: 0o600 });
  return output;
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
    console.log("Use login, init, build, dev, publish, or export. Options include --dir, --url, --name, --port, and --out. Export --with-data includes live data, accounts, and files.");
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
    if (!email || !password) throw Error("Set LITTLEBOX_EMAIL and LITTLEBOX_PASSWORD. Set LITTLEBOX_SETUP_KEY only when creating an owner.");
    const token = await login(url, email, password, process.env.LITTLEBOX_SETUP_KEY);
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
    await writeFile(path.join(root, "littlebox.json"), JSON.stringify({ url, project: metadata }, null, 2), { flag: "wx" });
    await ignoreInGit(root, [".littlebox/", "node_modules/"]);
    console.log(`Created ${project.name} in ${root}.`);
    return;
  }
  if (command === "build") { await buildLocal(root); console.log("Built .littlebox/artifact.json"); return; }
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
    const token = await login(url, "local-owner@littlebox.test", secret, secret);
    const devConfig = { ...config, url };
    try {
      const first = await publishLocal(root, devConfig, token);
      // A new private deployment starts with signup closed. The development Kernel is
      // isolated and local, so let the developer create test accounts.
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
  throw Error("Use login, init, build, dev, publish, or export. Options include --dir, --url, --name, --port, and --out.");
}

if (import.meta.main) main().catch(error => { console.error(error.message); process.exitCode = 1; });
