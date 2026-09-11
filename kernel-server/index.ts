import { Database } from "bun:sqlite";
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { actionDeclarations, parseServerActionSource, resolveServerAction } from "../src/kernel/actions";
import { createDeployment, normalizeManifest, type DeploymentVisibility } from "../src/deployments/manifest";
import { KernelIdentity } from "../src/kernel/auth";
import { LittleboxKernel } from "../src/kernel/kernel";
import { assertRuntimeDatabaseWithinLimits, assertRuntimeResultWithinLimits } from "../src/runtime/security";
import type { MiniAppProject } from "../src/lib/types";
import type { SqlValue } from "sql.js";
import { renderDeployment, renderPrivateLogin } from "./html";
import { DeploymentStore, SqliteKernelIdentityStore, type HostedDeployment } from "./store";
import { planDrizzleDatabaseChange } from "../database-lifecycle/server";
import type { PlanDatabaseChangeInput } from "../src/database-lifecycle/contracts";

const port = Number(process.env.LITTLEBOX_KERNEL_PORT ?? 8787);
const dataRoot = path.resolve(process.env.LITTLEBOX_DATA_DIR ?? "./data/kernel");
const adminKey = process.env.LITTLEBOX_ADMIN_KEY ?? "local-development-key";
const publicOrigin = process.env.LITTLEBOX_PUBLIC_ORIGIN ?? `http://localhost:${port}`;
const hostSuffix = safeHostSuffix(process.env.LITTLEBOX_HOST_SUFFIX ?? new URL(publicOrigin).hostname);
const secureCookie = new URL(publicOrigin).protocol === "https:" ? "; Secure" : "";
const sessionCookie = "littlebox_session";

await mkdir(dataRoot, { recursive: true });
const deployments = new DeploymentStore(path.join(dataRoot, "deployments"));
await deployments.initialize();
const identityStore = new SqliteKernelIdentityStore(path.join(dataRoot, "kernel.sqlite"));
const identity = new KernelIdentity(identityStore);
const kernel = new LittleboxKernel(identity);
const databases = new Map<string, Database>();

const server = Bun.serve({
  port,
  async fetch(request) {
    try {
      return await route(request);
    } catch (error) {
      console.error(error);
      const status = error instanceof KernelHttpError ? error.status : 500;
      return json({ error: error instanceof Error ? error.message : "Kernel request failed." }, status);
    }
  },
});

console.log(`Littlebox Kernel listening on ${server.url}`);

async function route(request: Request): Promise<Response> {
  const url = new URL(request.url);
  if (url.pathname === "/_littlebox/health") return json({ ok: true, kernel: "littlebox", version: 1 });
  if (url.pathname === "/_littlebox/admin/database-lifecycle/plan" && request.method === "POST") return planDatabaseMigration(request);
  if (url.pathname === "/_littlebox/admin/deploy" && request.method === "POST") return deploy(request);
  const adminDatabaseMatch = url.pathname.match(/^\/_littlebox\/admin\/deployments\/([^/]+)\/database(?:\/(export))?$/);
  if (adminDatabaseMatch) {
    const deployment = deployments.findById(decodeURIComponent(adminDatabaseMatch[1]));
    if (!deployment) return json({ error: "Deployment not found." }, 404);
    if (adminDatabaseMatch[2] === "export" && request.method === "GET") return exportHostedDatabase(request, deployment);
    if (request.method === "POST") return adminDatabase(request, deployment);
  }

  const deployment = deployments.findByHostname(hostname(request));
  if (!deployment) return new Response("Littlebox deployment not found.", { status: 404 });
  if (url.pathname === "/_littlebox/login") return request.method === "POST" ? login(request, deployment) : new Response(renderPrivateLogin(deployment), html());
  if (url.pathname === "/_littlebox/logout" && request.method === "POST") return logout(request);
  if (url.pathname === "/_littlebox/session") return session(request);
  if (url.pathname === "/_littlebox/files" && request.method === "GET") return listFiles(request, deployment);
  if (url.pathname === "/_littlebox/files" && request.method === "POST") return uploadFile(request, deployment);
  if (url.pathname.startsWith("/_littlebox/files/") && request.method === "GET") {
    return serveFile(request, deployment, url.pathname.slice("/_littlebox/files/".length));
  }
  if (url.pathname.startsWith("/_littlebox/actions/") && request.method === "POST") {
    return invokeAction(request, deployment, decodeURIComponent(url.pathname.slice("/_littlebox/actions/".length)));
  }

  const decision = await kernel.authorize(readCookie(request, sessionCookie), { type: "open-deployment", deployment });
  if (!decision.allowed) return new Response(renderPrivateLogin(deployment), html(401));
  if (url.pathname !== "/") return new Response("Not found", { status: 404 });
  return new Response(renderDeployment(deployment), html());
}

async function planDatabaseMigration(request: Request): Promise<Response> {
  if (!isAdmin(request)) return json({ error: "Admin authorization required." }, 401);
  const input = await readJson<PlanDatabaseChangeInput>(request, 120_000);
  return json({ result: await planDrizzleDatabaseChange(input) });
}

async function deploy(request: Request): Promise<Response> {
  if (!isAdmin(request)) return json({ error: "Admin authorization required." }, 401);
  const body = await readJson<{ project: MiniAppProject; visibility: DeploymentVisibility }>(request, 1_000_000);
  if (!body.project || (body.visibility !== "public" && body.visibility !== "private")) return json({ error: "Invalid deployment request." }, 400);
  const actionProgram = parseServerActionSource(body.project.files["actions.json"]);
  const manifest = normalizeManifest({
    ...body.project.manifest,
    visibility: body.visibility,
    runtime: {
      ...body.project.manifest.runtime,
      database: { mode: "declared-actions", actions: actionDeclarations(actionProgram) },
    },
  });
  const base = createDeployment({ ...body.project, manifest }, body.visibility);
  const shortId = base.id.replaceAll("-", "").slice(0, 6);
  const hostname = `${safeSlug(body.project.slug)}-${shortId}.${hostSuffix}`;
  const accessCode = body.visibility === "private" ? createAccessCode() : undefined;
  const hosted: HostedDeployment = {
    ...base,
    hostname,
    url: deploymentUrl(hostname),
    files: body.project.files,
    accessCodeHash: accessCode ? await sha256(accessCode) : undefined,
  };

  await deployments.save(hosted);
  const db = openDatabase(hosted);
  if (hosted.files["schema.sql"].trim()) db.exec(hosted.files["schema.sql"]);
  return json({ result: { ...publicDeployment(hosted), accessCode } }, 201);
}

async function invokeAction(request: Request, deployment: HostedDeployment, actionName: string) {
  const decision = await kernel.authorize(readCookie(request, sessionCookie), { type: "invoke-action", deployment, actionName });
  if (!decision.allowed) return json({ error: decision.code }, decision.code === "authentication-required" ? 401 : 403);
  const input = await readJson<unknown>(request, 64_000);
  const program = parseServerActionSource(deployment.files["actions.json"]);
  const { action, params } = resolveServerAction(program, actionName, input, { actorId: decision.actorId });
  const db = openDatabase(deployment);
  if (action.kind === "query") {
    const rows = db.query(action.statement).all(...params) as Record<string, string | number | Uint8Array | null>[];
    assertRuntimeResultWithinLimits(rows);
    return json({ result: rows });
  }

  const transaction = db.transaction(() => {
    const result = db.query(action.statement).run(...params);
    const pageCount = Number(db.query("PRAGMA page_count").get()?.page_count ?? 0);
    const pageSize = Number(db.query("PRAGMA page_size").get()?.page_size ?? 0);
    assertRuntimeDatabaseWithinLimits(pageCount * pageSize);
    return { changes: result.changes, lastInsertId: Number(result.lastInsertRowid) };
  });
  return json({ result: transaction() });
}

async function adminDatabase(request: Request, deployment: HostedDeployment) {
  if (!isAdmin(request)) return json({ error: "Admin authorization required." }, 401);
  const body = await readJson<Record<string, unknown>>(request, 200_000);
  const db = openDatabase(deployment);
  const operation = String(body.operation ?? "");
  if (operation === "inspect") {
    const names = db.query<{ name: string }, []>("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name").all();
    return json({ result: names.map(({ name }) => ({
      name,
      columns: describeTable(db, name).map((column) => column.name),
      totalRows: Number(db.query(`SELECT COUNT(*) AS count FROM ${quoteIdentifier(name)}`).get()?.count ?? 0),
    })) });
  }
  if (operation === "browse") {
    const tableName = String(body.tableName ?? "");
    const columns = describeTable(db, tableName);
    const pageSize = Math.min(Math.max(Number(body.pageSize ?? 50), 1), 100);
    const page = Math.max(Number(body.page ?? 0), 0);
    const requestedSort = String(body.sortColumn ?? "");
    const sort = columns.some((column) => column.name === requestedSort) ? quoteIdentifier(requestedSort) : "rowid";
    const direction = body.sortDirection === "DESC" ? "DESC" : "ASC";
    const rows = db.query(`SELECT rowid AS __littlebox_rowid, * FROM ${quoteIdentifier(tableName)} ORDER BY ${sort} ${direction} LIMIT ? OFFSET ?`)
      .all(pageSize, page * pageSize) as Record<string, SqlValue>[];
    const totalRows = Number(db.query(`SELECT COUNT(*) AS count FROM ${quoteIdentifier(tableName)}`).get()?.count ?? 0);
    return json({ result: {
      columns,
      rows: rows.map((row) => ({ rowId: jsonSqlValue(row.__littlebox_rowid), values: Object.fromEntries(Object.entries(row).filter(([key]) => key !== "__littlebox_rowid").map(([key, value]) => [key, jsonSqlValue(value)])) })),
      totalRows,
      page,
      pageSize,
    } });
  }
  if (operation === "sql") return adminSql(db, String(body.sql ?? ""));
  if (operation === "update-cell") {
    const tableName = String(body.tableName ?? "");
    const columnName = String(body.columnName ?? "");
    const columns = describeTable(db, tableName);
    if (!columns.some((column) => column.name === columnName)) return json({ error: "Unknown column." }, 400);
    adminWrite(db, () => db.query(`UPDATE ${quoteIdentifier(tableName)} SET ${quoteIdentifier(columnName)} = ? WHERE rowid = ?`).run(toSqlValue(body.value), toSqlValue(body.rowId)));
    return json({ result: { ok: true } });
  }
  if (operation === "insert-row") {
    const tableName = String(body.tableName ?? "");
    const columns = describeTable(db, tableName);
    const values = body.values && typeof body.values === "object" && !Array.isArray(body.values) ? body.values as Record<string, unknown> : {};
    const names = Object.keys(values);
    if (names.some((name) => !columns.some((column) => column.name === name))) return json({ error: "Unknown column." }, 400);
    adminWrite(db, () => {
      if (names.length === 0) db.exec(`INSERT INTO ${quoteIdentifier(tableName)} DEFAULT VALUES`);
      else db.query(`INSERT INTO ${quoteIdentifier(tableName)} (${names.map(quoteIdentifier).join(", ")}) VALUES (${names.map(() => "?").join(", ")})`)
        .run(...names.map((name) => toSqlValue(values[name])));
    });
    return json({ result: { ok: true } });
  }
  if (operation === "delete-row") {
    const tableName = String(body.tableName ?? "");
    describeTable(db, tableName);
    adminWrite(db, () => db.query(`DELETE FROM ${quoteIdentifier(tableName)} WHERE rowid = ?`).run(toSqlValue(body.rowId)));
    return json({ result: { ok: true } });
  }
  return json({ error: "Unknown database operation." }, 400);
}

function adminSql(db: Database, sql: string) {
  if (!sql.trim()) return json({ error: "Write a SQL statement first." }, 400);
  if (sql.length > 100_000) return json({ error: "SQL script is too large." }, 413);
  const started = performance.now();
  const firstWord = sql.trim().match(/^[a-z]+/i)?.[0].toUpperCase() ?? "SQL";
  if (["SELECT", "PRAGMA", "EXPLAIN", "WITH"].includes(firstWord)) {
    const rows = db.query(sql).all() as Record<string, SqlValue>[];
    assertRuntimeResultWithinLimits(rows);
    const normalized = rows.map((row) => Object.fromEntries(Object.entries(row).map(([key, value]) => [key, jsonSqlValue(value)])));
    return json({ result: { columns: normalized[0] ? Object.keys(normalized[0]) : [], rows: normalized, changes: 0, elapsedMs: performance.now() - started, statement: firstWord } });
  }
  const result = adminWrite(db, () => db.query(sql).run());
  return json({ result: { columns: [], rows: [], changes: Number(result.changes), elapsedMs: performance.now() - started, statement: firstWord } });
}

function exportHostedDatabase(request: Request, deployment: HostedDeployment) {
  if (!isAdmin(request)) return json({ error: "Admin authorization required." }, 401);
  const bytes = openDatabase(deployment).serialize();
  return new Response(bytes, { headers: { "content-type": "application/vnd.sqlite3", "content-disposition": `attachment; filename="${safeSlug(deployment.projectName)}.sqlite"`, "cache-control": "no-store" } });
}

async function listFiles(request: Request, deployment: HostedDeployment) {
  const decision = await kernel.authorize(readCookie(request, sessionCookie), { type: "open-deployment", deployment });
  if (!decision.allowed) return json({ error: decision.code }, decision.code === "authentication-required" ? 401 : 403);
  return json({ result: await deployments.listFiles(deployment.id) });
}

async function uploadFile(request: Request, deployment: HostedDeployment) {
  const decision = await kernel.authorize(readCookie(request, sessionCookie), { type: "open-deployment", deployment });
  if (!decision.allowed) return json({ error: decision.code }, decision.code === "authentication-required" ? 401 : 403);
  const requestBytes = Number(request.headers.get("content-length") ?? 0);
  if (requestBytes > 2_200_000) return json({ error: "Files are limited to 2 MiB." }, 413);
  const form = await request.formData();
  const candidate = form.get("file");
  if (!(candidate instanceof File)) return json({ error: "A file is required." }, 400);
  if (candidate.size <= 0 || candidate.size > 2 * 1024 * 1024) return json({ error: "Files are limited to 2 MiB." }, 413);
  const id = crypto.randomUUID();
  const file = {
    id,
    name: safeFileName(candidate.name),
    type: safeContentType(candidate.type),
    size: candidate.size,
    url: `/_littlebox/files/${id}`,
    createdAt: Date.now(),
  };
  try {
    await deployments.saveFile(deployment.id, file, new Uint8Array(await candidate.arrayBuffer()), 20 * 1024 * 1024);
  } catch (error) {
    if (error instanceof Error && error.message.includes("file quota")) return json({ error: "This deployment has reached its 20 MiB file quota." }, 413);
    throw error;
  }
  return json({ result: file }, 201);
}

async function serveFile(request: Request, deployment: HostedDeployment, fileId: string) {
  const decision = await kernel.authorize(readCookie(request, sessionCookie), { type: "open-deployment", deployment });
  if (!decision.allowed) return new Response("Forbidden", { status: 403 });
  if (!/^[0-9a-f-]{36}$/i.test(fileId)) return new Response("Not found", { status: 404 });
  const file = await deployments.readFile(deployment.id, fileId);
  if (!file) return new Response("Not found", { status: 404 });
  const inline = /^(image\/(?:png|jpeg|gif|webp|avif))$/i.test(file.metadata.type);
  return new Response(file.bytes, {
    headers: {
      "content-type": file.metadata.type,
      "content-length": String(file.metadata.size),
      "cache-control": "private, max-age=31536000, immutable",
      "content-disposition": `${inline ? "inline" : "attachment"}; filename*=UTF-8''${encodeURIComponent(file.metadata.name)}`,
      "content-security-policy": "default-src 'none'; sandbox",
      "cross-origin-resource-policy": "same-origin",
      "x-content-type-options": "nosniff",
    },
  });
}

async function login(request: Request, deployment: HostedDeployment) {
  if (deployment.manifest.visibility !== "private" || !deployment.accessCodeHash) return new Response("Login is not required.", { status: 400 });
  const form = await request.formData();
  const code = String(form.get("code") ?? "");
  if (!constantTimeEqual(await sha256(code), deployment.accessCodeHash)) {
    return new Response(renderPrivateLogin(deployment, "That access code is not valid."), html(401));
  }
  const actorId = `actor_${crypto.randomUUID()}`;
  identityStore.grant(actorId, { type: "deployment", deploymentId: deployment.id });
  const session = await identity.issueSession({ actorId, ttlMs: 7 * 86_400_000 });
  return new Response(null, {
    status: 303,
    headers: {
      location: "/",
      "set-cookie": `${sessionCookie}=${session.token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${7 * 86_400}${secureCookie}`,
    },
  });
}

async function logout(request: Request) {
  const token = readCookie(request, sessionCookie);
  if (token) await identity.revokeSession(token);
  return new Response(null, { status: 303, headers: { location: "/", "set-cookie": `${sessionCookie}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0${secureCookie}` } });
}

async function session(request: Request) {
  const actor = await identity.authenticate(readCookie(request, sessionCookie));
  return json({ result: actor
    ? { userId: actor.actorId, name: "Littlebox member", role: "member" }
    : { userId: "anonymous", name: "Anonymous", role: "anonymous" } });
}

function openDatabase(deployment: HostedDeployment) {
  const existing = databases.get(deployment.id);
  if (existing) return existing;
  const db = new Database(deployments.databasePath(deployment.id), { create: true, strict: true });
  db.exec("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;");
  databases.set(deployment.id, db);
  return db;
}

function publicDeployment(deployment: HostedDeployment) {
  const { accessCodeHash: _secret, files: _files, hostname: _hostname, ...record } = deployment;
  return record;
}

function hostname(request: Request) {
  return (request.headers.get("x-forwarded-host") ?? request.headers.get("host") ?? "").split(":")[0].toLowerCase();
}

function readCookie(request: Request, name: string) {
  const pair = request.headers.get("cookie")?.split(";").map((value) => value.trim()).find((value) => value.startsWith(`${name}=`));
  return pair ? decodeURIComponent(pair.slice(name.length + 1)) : null;
}

function isAdmin(request: Request) {
  return request.headers.get("x-littlebox-admin-key") === adminKey;
}

function describeTable(db: Database, tableName: string) {
  const exists = db.query("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?").get(tableName);
  if (!exists) throw new Error(`Table not found: ${tableName}`);
  return db.query<{ name: string; type: string; notnull: number; dflt_value: SqlValue; pk: number }, []>(
    `PRAGMA table_info(${quoteIdentifier(tableName)})`,
  ).all().map((column) => ({
    name: column.name,
    type: column.type || "ANY",
    notNull: Boolean(column.notnull),
    defaultValue: jsonSqlValue(column.dflt_value),
    primaryKey: Boolean(column.pk),
  }));
}

function quoteIdentifier(value: string) {
  return `"${value.replaceAll('"', '""')}"`;
}

function jsonSqlValue(value: unknown): string | number | null {
  if (value === null || value === undefined) return null;
  if (typeof value === "number" || typeof value === "string") return value;
  if (typeof value === "bigint") return value.toString();
  if (value instanceof Uint8Array) return `[blob · ${value.byteLength} bytes]`;
  return String(value);
}

function toSqlValue(value: unknown): string | number | null {
  if (value === null || value === undefined) return null;
  if (typeof value === "string" || typeof value === "number") return value;
  if (typeof value === "boolean") return value ? 1 : 0;
  throw new Error("Unsupported SQLite value.");
}

function assertDatabaseSize(db: Database) {
  const pageCount = Number(db.query("PRAGMA page_count").get()?.page_count ?? 0);
  const pageSize = Number(db.query("PRAGMA page_size").get()?.page_size ?? 0);
  assertRuntimeDatabaseWithinLimits(pageCount * pageSize);
}

function adminWrite<T>(db: Database, write: () => T) {
  return db.transaction(() => {
    const result = write();
    assertDatabaseSize(db);
    return result;
  })();
}

async function readJson<T>(request: Request, maxBytes: number): Promise<T> {
  const length = Number(request.headers.get("content-length") ?? 0);
  if (length > maxBytes) throw new KernelHttpError(413, "Request body is too large.");
  const text = await request.text();
  if (new TextEncoder().encode(text).byteLength > maxBytes) throw new KernelHttpError(413, "Request body is too large.");
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new KernelHttpError(400, "Request body must be valid JSON.");
  }
}

function json(value: unknown, status = 200) {
  return Response.json(value, { status, headers: { "cache-control": "no-store", "x-content-type-options": "nosniff" } });
}

function html(status = 200): ResponseInit {
  return { status, headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-store", "content-security-policy": "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'", "x-content-type-options": "nosniff", "referrer-policy": "no-referrer" } };
}

function safeSlug(value: string) {
  const slug = value.toLowerCase().replace(/[^a-z0-9-]/g, "-").replace(/-+/g, "-").replace(/^-|-$/g, "");
  if (!slug) throw new Error("Project slug is invalid.");
  return slug.slice(0, 45);
}

function safeHostSuffix(value: string) {
  const suffix = value.toLowerCase().replace(/^\.+|\.+$/g, "");
  if (!/^[a-z0-9.-]+$/.test(suffix) || suffix.includes("..")) throw new Error("LITTLEBOX_HOST_SUFFIX is invalid.");
  return suffix;
}

function deploymentUrl(hostname: string) {
  if (publicOrigin.includes("{hostname}")) return publicOrigin.replaceAll("{hostname}", hostname);
  const url = new URL(publicOrigin);
  url.hostname = hostname;
  return url.toString().replace(/\/$/, "");
}

function createAccessCode() {
  return Array.from(crypto.getRandomValues(new Uint8Array(12)), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function safeFileName(value: string) {
  const name = value.replace(/[\u0000-\u001f\u007f]/g, "").replace(/[\\/]/g, "-").trim();
  return (name || "file").slice(0, 180);
}

function safeContentType(value: string) {
  return /^[a-z0-9.+-]+\/[a-z0-9.+-]+$/i.test(value) ? value.toLowerCase() : "application/octet-stream";
}

async function sha256(value: string) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value)));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function constantTimeEqual(left: string, right: string) {
  if (left.length !== right.length) return false;
  let difference = 0;
  for (let index = 0; index < left.length; index += 1) difference |= left.charCodeAt(index) ^ right.charCodeAt(index);
  return difference === 0;
}

class KernelHttpError extends Error {
  constructor(readonly status: number, message: string) {
    super(message);
  }
}
