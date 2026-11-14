// `konae push` is the inverse of `konae pull`: it imports the folder's data/
// into the app's own published Deployment with project_import replace=data,
// as `kernel import <folder>` imports it on a Kernel without Konae. Rows are
// never merged. Every check this machine can make runs before anything is
// sent, and the published app goes into maintenance only for the transfer
// and the import.
import { Database } from "bun:sqlite";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import type { McpClient, Transfer } from "../src/mcp/client";
import { packageComponentLimits, packageFormat, packageVersion, type PackageComponentName, type PackageContent, type PackageManifest } from "../src/lib/portable-package";
import { DATA_FOLDER } from "../src/lib/app-folder";
import { describeSchemaChange, type SchemaChange } from "../src/deployments/release";
import { checksumFile, uploadFromFile, writeTarFile } from "./package-files";

/** The per-file cap and the quota of one Deployment, as platform_describe answers them. */
export type FileLimits = { quotaBytes: number; maxFileBytes: number };
/** The Kernel's defaults, for an installation whose platform_describe predates `files`. */
const DEFAULT_LIMITS: FileLimits = { quotaBytes: 20 * 1024 * 1024, maxFileBytes: 2 * 1024 * 1024 };

/** One file `data/files/index.json` lists, with the blob beside it. */
export type IndexedFile = { id: string; name: string; size: number; blob: string };

/** A component ready to upload: a file in the scratch folder, its size and SHA-256. */
export type Staged = { name: PackageComponentName; file: string; size: number; sha256: string };

/** What push found in data/ and what it will send. */
export type LocalData = {
  components: Staged[];
  tables: { name: string; rows: number }[];
  accounts?: number;
  files?: { count: number; bytes: number };
};

const NOTHING_SENT = "Nothing was sent.";

/** Sizes as the Kernel writes them: whole MiB or KiB when exact, else one decimal. */
export function bytes(value: number) {
  const mib = 1024 * 1024;
  if (value >= mib) return value % mib === 0 ? `${value / mib} MiB` : `${(value / mib).toFixed(1)} MiB`;
  if (value >= 1024) return value % 1024 === 0 ? `${value / 1024} KiB` : `${(value / 1024).toFixed(1)} KiB`;
  return value === 1 ? "1 byte" : `${value} bytes`;
}

/** The installation's file limits from platform_describe. */
export async function fileLimits(client: Pick<McpClient, "call">): Promise<FileLimits> {
  const described = await client.call<{ files?: Partial<FileLimits> }>("platform_describe");
  const { quotaBytes, maxFileBytes } = described.files ?? {};
  return Number.isSafeInteger(quotaBytes) && Number.isSafeInteger(maxFileBytes) ? { quotaBytes: quotaBytes!, maxFileBytes: maxFileBytes! } : DEFAULT_LIMITS;
}

/**
 * Reads `data/files/index.json` and checks each file as the Kernel's import
 * does: a valid id, a blob beside the index whose size is the indexed size,
 * an owner the pushed accounts hold, the per-file cap and the quota. A
 * refusal names the largest offending file and the totals.
 */
export async function checkFiles(directory: string, limits: FileLimits, accounts: Set<string> | undefined): Promise<{ index: Uint8Array; files: IndexedFile[] }> {
  const where = path.join(DATA_FOLDER, "files", "index.json");
  let index: Uint8Array;
  try { index = new Uint8Array(await readFile(path.join(directory, "index.json"))); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return { index: new TextEncoder().encode("[]"), files: [] };
    throw error;
  }
  let parsed: unknown;
  try { parsed = JSON.parse(new TextDecoder().decode(index)); } catch { throw Error(`${where} is not valid JSON. ${NOTHING_SENT}`); }
  if (!Array.isArray(parsed)) throw Error(`${where} must be a JSON array of files. ${NOTHING_SENT}`);
  const files: IndexedFile[] = [];
  const seen = new Set<string>();
  for (const entry of parsed as Record<string, unknown>[]) {
    const id = entry?.id;
    if (typeof id !== "string" || id.length !== 36 || !/^[0-9a-fA-F-]+$/.test(id)) throw Error(`${where} lists a file whose id is not 36 hexadecimal digits and dashes: ${JSON.stringify(id)}. ${NOTHING_SENT}`);
    if (seen.has(id)) throw Error(`${where} lists file ${id} twice. ${NOTHING_SENT}`);
    seen.add(id);
    const size = entry.size;
    if (!Number.isSafeInteger(size) || (size as number) < 0) throw Error(`${where} gives file ${id} no size in bytes. ${NOTHING_SENT}`);
    const blob = path.join(directory, `${id}.blob`);
    const actual = await stat(blob).then((info) => info.size, () => undefined);
    if (actual === undefined) throw Error(`${where} lists file ${id}, but ${path.join(DATA_FOLDER, "files", `${id}.blob`)} is missing. Put the bytes there or remove the entry. ${NOTHING_SENT}`);
    if (actual !== size) throw Error(`${path.join(DATA_FOLDER, "files", `${id}.blob`)} is ${actual} bytes, but ${where} says ${size}. Make the index give its real size. ${NOTHING_SENT}`);
    const owner = entry.ownerId;
    if (owner !== undefined && owner !== null) {
      if (typeof owner !== "string" || !/^[A-Za-z0-9_-]{1,80}$/.test(owner)) throw Error(`${where} gives file ${id} an invalid ownerId. ${NOTHING_SENT}`);
      if (!accounts?.has(owner)) {
        const where = accounts ? `${path.join(DATA_FOLDER, "identity.sqlite")} does not hold` : `is in no ${path.join(DATA_FOLDER, "identity.sqlite")}, and the import checks owners against the accounts it receives`;
        throw Error(`File ${id} belongs to account ${owner}, which ${where}. Add the account, or remove ownerId from the file. ${NOTHING_SENT}`);
      }
    }
    files.push({ id, name: typeof entry.name === "string" ? entry.name : "file", size: size as number, blob });
  }
  const total = files.reduce((sum, file) => sum + file.size, 0);
  const totals = `${files.length} file${files.length === 1 ? "" : "s"} in ${where} come to ${bytes(total)}`;
  const oversized = files.filter((file) => file.size > limits.maxFileBytes).sort((a, b) => b.size - a.size);
  if (oversized.length > 0) {
    const largest = oversized[0];
    throw Error([
      `${oversized.length === 1 ? "One file is" : `${oversized.length} files are`} over this installation's limit of ${bytes(limits.maxFileBytes)} per file (KONAE_FILE_MAX_BYTES).`,
      `The largest is ${largest.name} (${largest.id}) at ${bytes(largest.size)}.`,
      `${totals}, and the quota is ${bytes(limits.quotaBytes)} per app (KONAE_FILES_QUOTA_BYTES).`,
      `Remove ${oversized.length === 1 ? "its entry" : "their entries"} from ${where} and ${oversized.length === 1 ? "its" : "their"} <id>.blob beside it, or ask the operator to raise KONAE_FILE_MAX_BYTES. ${NOTHING_SENT}`,
    ].join(" "));
  }
  if (total > limits.quotaBytes) {
    const largest = [...files].sort((a, b) => b.size - a.size)[0];
    throw Error([
      `${totals}, over this installation's quota of ${bytes(limits.quotaBytes)} per app (KONAE_FILES_QUOTA_BYTES).`,
      `The largest is ${largest.name} (${largest.id}) at ${bytes(largest.size)}.`,
      `Remove entries from ${where} with their <id>.blob files, or ask the operator to raise KONAE_FILES_QUOTA_BYTES. ${NOTHING_SENT}`,
    ].join(" "));
  }
  return { index, files };
}

/** Rows per table of an app database, as database_execute inspect counts them. */
function countRows(file: string) {
  const db = new Database(file, { readonly: true });
  try {
    const names = db.query<{ name: string }, []>("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").all().map((row) => row.name);
    return names.map((name) => ({ name, rows: db.query<{ n: number }, []>(`SELECT COUNT(*) AS n FROM "${name.replaceAll('"', '""')}"`).get()!.n }));
  } finally { db.close(); }
}

/**
 * A consistent copy of a SQLite file, rows still in its write-ahead log
 * included. The connection opens read-write without creating, because a
 * read-only one cannot make the -shm file a WAL database needs once the
 * Kernel has closed it. VACUUM INTO changes nothing in the source.
 */
function snapshot(source: string, target: string) {
  const db = new Database(source, { readwrite: true, create: false });
  try { db.run("VACUUM INTO ?", [target]); } finally { db.close(); }
}

async function exists(file: string) { return stat(file).then(() => true, () => false); }

/**
 * Stages the parts of `data/` the Kernel's folder import takes: database,
 * identity when present, files when `data/files` exists, and usage when
 * present. `secrets.sqlite` never leaves. Checks every limit on the way.
 */
export async function stageData(root: string, scratch: string, limits: FileLimits): Promise<LocalData> {
  const data = path.join(root, DATA_FOLDER);
  const source = (name: string) => path.join(data, name);
  if (!(await exists(source("database.sqlite")))) throw Error(`${path.join(DATA_FOLDER, "database.sqlite")} does not exist in ${root}, so there is no data to push. Run dev, pull and dev --reset, or write data/ as docs/guides/bring-existing-data.md says.`);
  const components: Staged[] = [];
  const add = async (name: PackageComponentName, file: string) => {
    const sized = await checksumFile(file);
    // The files archive is bounded by the installation's quota, which checkFiles applies.
    const limit = name === "files" ? Infinity : packageComponentLimits[name];
    if (sized.size > limit) throw Error(`The ${name} component is ${bytes(sized.size)}; the Kernel takes at most ${bytes(limit)}. ${NOTHING_SENT}`);
    components.push({ name, file, ...sized });
  };
  const result: LocalData = { components, tables: [] };
  let accounts: Set<string> | undefined;
  for (const [name, file] of [["database", "database.sqlite"], ["identity", "identity.sqlite"], ["usage", "usage.sqlite"]] as const) {
    if (!(await exists(source(file)))) continue;
    const copy = path.join(scratch, file);
    snapshot(source(file), copy);
    if (name === "database") result.tables = countRows(copy);
    if (name === "identity") {
      const db = new Database(copy, { readonly: true });
      try {
        const has = db.query("SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_users'").get();
        if (!has) throw Error(`${path.join(DATA_FOLDER, "identity.sqlite")} has no app_users table, so it is not an accounts file. ${NOTHING_SENT}`);
        accounts = new Set(db.query<{ id: string }, []>("SELECT id FROM app_users").all().map((row) => row.id));
        result.accounts = accounts.size;
      } finally { db.close(); }
    }
    await add(name, copy);
  }
  if (await stat(source("files")).then((info) => info.isDirectory(), () => false)) {
    const { index, files } = await checkFiles(source("files"), limits, accounts);
    // index.json first, then each blob in index order: the order the Kernel reads. Nothing else in the folder goes.
    const archive = path.join(scratch, "files.tar");
    await writeTarFile(archive, [{ path: "index.json", contents: index, mode: 0o600 }, ...files.map((file) => ({ path: `${file.id}.blob`, file: file.blob, size: file.size }))]);
    await add("files", archive);
    result.files = { count: files.length, bytes: files.reduce((sum, file) => sum + file.size, 0) };
  }
  // The manifest lists database first, as an export does.
  components.sort((a, b) => order.indexOf(a.name) - order.indexOf(b.name));
  return result;
}
const order: PackageComponentName[] = ["database", "identity", "files", "usage"];

/** The manifest of a package built from data/, for `project_import`. */
export function pushManifest(deployment: { id: string; projectId: string }, visibility: "public" | "private", components: Staged[]): PackageManifest {
  return {
    format: packageFormat, version: packageVersion, packageId: crypto.randomUUID(), createdAt: Date.now(),
    deploymentId: deployment.id, projectId: deployment.projectId, visibility, publishedRevision: 0,
    contents: components.map(({ name, size, sha256 }): PackageContent => ({ name, size, sha256 })), excluded: [],
  };
}

type Inspected = { name: string; totalRows: number }[];
async function remoteRows(client: Pick<McpClient, "call">, deploymentId: string) {
  const answer = await client.call<Inspected | { result: Inspected }>("database_execute", { deploymentId, operation: "inspect" });
  return new Map((Array.isArray(answer) ? answer : answer.result).map((table) => [table.name, table.totalRows]));
}

/** Local rows against published rows, one line per table either side has. */
export function rowsTable(local: { name: string; rows: number }[], remote: Map<string, number> | undefined, headings: [string, string]) {
  const names = [...new Set([...local.map((table) => table.name), ...(remote?.keys() ?? [])])].sort();
  const here = new Map(local.map((table) => [table.name, table.rows]));
  const width = Math.max(5, ...names.map((name) => name.length));
  const cell = (value: number | undefined) => String(value ?? "-").padStart(10);
  return [`  ${"table".padEnd(width)} ${headings[0].padStart(10)} ${headings[1].padStart(10)}`,
    ...names.map((name) => `  ${name.padEnd(width)} ${cell(here.get(name))} ${remote ? cell(remote.get(name)) : "".padStart(10)}`)];
}

export type PushOptions = { yes: boolean; confirmSchema?: string; log?: (line: string) => void };

export type Pushed = { backupId: string; deploymentId: string; leftInMaintenance: boolean };

/**
 * Replaces the published app's data with the folder's `data/`. Without
 * `yes` it prints what would be replaced and refuses. With it: maintenance
 * on (unless the owner already put the app there), each component uploaded
 * in the installation's block size, project_import replace=data, which
 * writes its own backup first, the rows read back, and maintenance back to
 * how it was, on every failure path too.
 */
export async function pushData(root: string, client: McpClient, project: { id: string; name: string; visibility: "public" | "private" }, options: PushOptions): Promise<Pushed> {
  const log = options.log ?? console.log;
  const inspected = await client.call<{ deployment?: { id: string; url?: string; projectId?: string; manifest?: { visibility?: string } } | null }>("project_inspect", { projectId: project.id });
  const deployment = inspected.deployment;
  if (!deployment) throw Error("This project has no Deployment to push to. Publish it first.");
  const limits = await fileLimits(client);
  const scratch = await mkdtemp(path.join(tmpdir(), "konae-push-"));
  const transfers: Transfer[] = [];
  try {
    const local = await stageData(root, scratch, limits);
    const before = await remoteRows(client, deployment.id);
    const sent = local.components.map((component) => component.name);
    log(`push replaces the data of ${project.name} at ${deployment.url ?? deployment.id} with ${path.join(root, DATA_FOLDER)}. Rows are never merged.`);
    for (const line of rowsTable(local.tables, before, ["here", "published"])) log(line);
    log(`  accounts: ${local.accounts === undefined ? "no identity.sqlite here, so the published accounts stay"
      : local.accounts === 0 ? "identity.sqlite here holds none, so every published account goes. Move identity.sqlite out of data/ to keep them"
      : `${local.accounts} here replace the published ones`}`);
    log(`  files: ${local.files ? `${local.files.count} here (${bytes(local.files.bytes)}) replace the published ones` : "no data/files here, so the published files stay"}`);
    log(`  usage: ${sent.includes("usage") ? "usage.sqlite here replaces the published Usage and Signals" : "no usage.sqlite here, so the published Usage and Signals are removed"}`);
    log(`It sends ${local.components.map((component) => `${component.name} ${bytes(component.size)}`).join(", ")}. The app keeps its sessions, API keys, Signup Policy, sign-in methods, secrets and address.`);
    if (!options.yes) throw Error(`${NOTHING_SENT} To replace the published data with this, run push again with --yes.`);

    const paused = (await client.call<{ maintenance?: boolean }>("maintenance_read", { deploymentId: deployment.id })).maintenance === true;
    let backupId: string | undefined;
    let failure: unknown;
    // True once push itself put the app in maintenance, so only push's own pause is lifted.
    let pausedHere = false;
    // An interrupt would skip the finally below, so it releases the uploads and lifts the pause itself.
    const interrupted = () => {
      log(pausedHere ? "Interrupted. Taking the app out of maintenance." : "Interrupted.");
      void Promise.all(transfers.map((transfer) => client.cancel(transfer.transferId).catch(() => {})))
        .then(() => pausedHere ? client.call("maintenance_set", { deploymentId: deployment.id, on: false }) : undefined)
        .catch(() => {}).finally(() => process.exit(130));
    };
    process.once("SIGINT", interrupted);
    try {
      if (!paused) {
        await client.call("maintenance_set", { deploymentId: deployment.id, on: true });
        pausedHere = true;
        log("Put the app in maintenance. It serves a maintenance page and refuses writes until the import ends.");
      }
      const components: Record<string, string> = {};
      for (const component of local.components) {
        // Recorded as soon as it begins, so an interrupt mid-upload releases it too.
        const transfer = await uploadFromFile(client, component.file, component, { projectId: project.id, component: component.name }, (begun) => transfers.push(begun));
        components[component.name] = transfer.transferId;
        log(`Uploaded ${component.name} (${bytes(component.size)}).`);
      }
      const visibility = deployment.manifest?.visibility === "public" || deployment.manifest?.visibility === "private" ? deployment.manifest.visibility : project.visibility;
      const manifest = pushManifest({ id: deployment.id, projectId: project.id }, visibility, local.components);
      const answer = await client.call<{ imported: boolean; backup?: { id: string }; schema?: SchemaChange }>("project_import", {
        deploymentId: deployment.id, replace: "data", package: { manifest, components },
        ...(options.confirmSchema ? { confirmSchema: options.confirmSchema } : {}),
      });
      if (!answer.imported) {
        const change = answer.schema!;
        const [, ...plan] = describeSchemaChange(change);
        for (const line of ["Bringing data/ to the published schema loses data:", ...plan]) log(line);
        throw Error(`Nothing changed, because the published schema would drop what data/ holds. To apply this plan, run push again with --yes --confirm-schema ${change.confirmation}.`);
      }
      backupId = answer.backup!.id;
      log(`Imported. Backup ${backupId} holds the published data as it was before.`);
      // Read back through the same door. The backup is listed, and the published rows are the ones sent.
      const backups = await client.call<{ id: string }[] | { result: { id: string }[] }>("database_execute", { deploymentId: deployment.id, operation: "list-backups" });
      if (!(Array.isArray(backups) ? backups : backups.result).some((backup) => backup.id === backupId)) throw Error(`The import answered backup ${backupId}, but list-backups does not show it.`);
      const after = await remoteRows(client, deployment.id);
      for (const line of rowsTable(local.tables, after, ["sent", "published"])) log(line);
      const differ = local.tables.filter((table) => after.get(table.name) !== table.rows);
      if (differ.length > 0) log(`Warning: ${differ.map((table) => table.name).join(", ")} ${differ.length === 1 ? "has" : "have"} other row counts than were sent, because the published schema changed them.`);
    } catch (error) {
      failure = error;
    } finally {
      process.off("SIGINT", interrupted);
      for (const transfer of transfers) await client.cancel(transfer.transferId).catch(() => {});
      if (pausedHere) {
        const lifted = await client.call("maintenance_set", { deploymentId: deployment.id, on: false })
          .then(() => client.call<{ maintenance?: boolean }>("maintenance_read", { deploymentId: deployment.id }))
          .then((read) => read.maintenance === false, () => false);
        if (lifted) log("Took the app out of maintenance.");
        else log(`Warning: the app is still in maintenance. Take it out with maintenance_set { deploymentId: "${deployment.id}", on: false } and check with maintenance_read.`);
      }
    }
    if (failure) {
      const message = failure instanceof Error ? failure.message : String(failure);
      throw Error(backupId ? `${message} Backup ${backupId} holds the published data as it was before; database_execute restore-backup with that backupId brings it back.` : message);
    }
    if (paused) log("The app was in maintenance before push, so it stays there. Take it out with maintenance_set on false when you are ready.");
    return { backupId: backupId!, deploymentId: deployment.id, leftInMaintenance: paused };
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}
