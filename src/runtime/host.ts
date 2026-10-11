import initSqlJs, { type Database, type SqlJsStatic, type SqlValue } from "sql.js";
import wasmUrl from "sql.js/dist/sql-wasm.wasm?url";
import {
  assertRuntimeDatabaseWithinLimits,
  assertRuntimeResultWithinLimits,
  assertRuntimeSql,
  queryResultTooLarge,
  runtimeLimits,
  validateRuntimeParams,
} from "./security";
import { parseServerActionSource, resolveServerAction, routeCallers, type ServerActionProgram } from "../kernel/actions";
import { thenOf } from "../kernel/calls";
import type { ApplyDatabaseMigrationInput } from "../database-lifecycle/contracts";

export type StoredFile = {
  id: string;
  name: string;
  type: string;
  size: number;
  url: string;
  createdAt: number;
  visibility?: "private" | "public";
  ownerId?: string;
};

/** A table as the studio sidebar knows it: name, shape and size — never contents. */
export type DatabaseTable = {
  name: string;
  columns: string[];
  totalRows: number;
};

export type DatabaseRunResult = {
  columns: string[];
  rows: Record<string, SqlValue>[];
  changes: number;
  elapsedMs: number;
  statement: string;
};

export type DatabaseColumn = {
  name: string;
  type: string;
  notNull: boolean;
  defaultValue: SqlValue;
  primaryKey: boolean;
};

export type DatabasePage = {
  columns: DatabaseColumn[];
  rows: Array<{ rowId: SqlValue; values: Record<string, SqlValue> }>;
  totalRows: number;
  page: number;
  pageSize: number;
};

export type DatabaseBackup = {
  id: string;
  label: string;
  createdAt: number;
  sizeBytes: number;
  databaseModelSource?: string;
  schemaRevision?: string;
};

export type RuntimeSession = {
  userId: string;
  name: string;
  role: string;
};

export type PreviewActor = "guest" | "alice" | "bob";

export type DatabaseAdminHost = {
  inspectDatabase(): Promise<DatabaseTable[]>;
  runSql(sql: string): Promise<DatabaseRunResult>;
  browseTable(tableName: string, options?: { page?: number; pageSize?: number; sortColumn?: string; sortDirection?: "ASC" | "DESC" }): Promise<DatabasePage>;
  updateCell(tableName: string, rowId: SqlValue, columnName: string, value: SqlValue): Promise<unknown>;
  insertRow(tableName: string, values: Record<string, SqlValue>): Promise<unknown>;
  deleteRow(tableName: string, rowId: SqlValue): Promise<unknown>;
  exportDatabase(): Promise<Uint8Array>;
  listDatabaseBackups?(): Promise<DatabaseBackup[]>;
  createDatabaseBackup?(label?: string, model?: { source: string; revision: string }): Promise<DatabaseBackup>;
  restoreDatabaseBackup?(backupId: string): Promise<DatabaseBackup>;
  applyDatabaseMigration?(input: ApplyDatabaseMigrationInput): Promise<{ backup: DatabaseBackup }>;
};

type StoredDatabaseBackup = DatabaseBackup & { database: string };

const localBackupLimit = 5;

let sqlPromise: Promise<SqlJsStatic> | undefined;

/** The tab's single sql.js instance. The Validation Gate borrows it instead of loading a second wasm heap. */
export function loadSqlJs(): Promise<SqlJsStatic> {
  sqlPromise ??= initSqlJs({ locateFile: () => wasmUrl }).catch((error) => {
    sqlPromise = undefined;
    throw error;
  });
  return sqlPromise;
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8_192) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8_192));
  }
  return btoa(binary);
}

function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

export type PreviewFailure = {
  kind: "error" | "unhandledrejection" | "actions" | "schema";
  message: string;
  stack: string;
  count: number;
};

const previewErrorLimit = 20;

/** Why the preview is not running the files as written. Null when it is. */
export type RuntimeDiagnostics = {
  actionsError: string | null;
  schemaError: string | null;
};

const emptyActionProgram: ServerActionProgram = { version: 1, actions: {} };

export class MiniAppRuntimeHost {
  private previewActor: PreviewActor = "guest";
  private readonly sessionListeners = new Set<(session: RuntimeSession) => void>();
  private readonly fileListeners = new Set<(files: StoredFile[]) => void>();
  private previewFailures: PreviewFailure[] = [];
  private readonly diagnosticListeners = new Set<(diagnostics: RuntimeDiagnostics) => void>();
  private dbPromise: Promise<Database>;
  /** The schema.sql the editor holds now, whether or not it ran. */
  private requestedSchema: string;
  /** The last schema.sql that ran without error. Only this one makes setSchema a no-op. */
  private appliedSchema: string | null = null;
  private schemaFailure: string | null = null;
  private actionProgram: ServerActionProgram = emptyActionProgram;
  private actionsFailure: string | null = null;

  /**
   * Never throws on bad Capsule files: the host is built during render, and a
   * throw there would take the whole editor down with it, on every reload. A
   * broken actions.json or schema.sql is reported through `diagnostics()`.
   */
  constructor(private readonly appId: string, schema: string, actions: string) {
    this.requestedSchema = schema;
    this.setActions(actions);
    this.dbPromise = this.openDatabase();
  }

  /** Resolves once the schema has run or its failure is recorded. Never rejects. */
  setSchema(schema: string): Promise<void> {
    if (schema === this.requestedSchema && schema === this.appliedSchema) return Promise.resolve();
    this.requestedSchema = schema;
    return this.dbPromise.then(
      (db) => this.applySchema(db, schema),
      (error) => this.recordSchemaFailure(schema, error),
    );
  }

  /**
   * Keeps the last program that parsed when the new source does not, so the
   * preview stays usable while the owner or the agent fixes the file.
   */
  setActions(actions: string) {
    try {
      this.actionProgram = parseServerActionSource(actions);
      this.setDiagnostics({ actionsError: null });
    } catch (error) {
      this.setDiagnostics({ actionsError: describeError(error) });
    }
  }

  get actionsError(): string | null {
    return this.actionsFailure;
  }

  get schemaError(): string | null {
    return this.schemaFailure;
  }

  diagnostics(): RuntimeDiagnostics {
    return { actionsError: this.actionsFailure, schemaError: this.schemaFailure };
  }

  subscribeDiagnostics(listener: (diagnostics: RuntimeDiagnostics) => void) {
    this.diagnosticListeners.add(listener);
    listener(this.diagnostics());
    return () => {
      this.diagnosticListeners.delete(listener);
    };
  }

  private setDiagnostics(change: Partial<RuntimeDiagnostics>) {
    const actionsError = change.actionsError === undefined ? this.actionsFailure : change.actionsError;
    const schemaError = change.schemaError === undefined ? this.schemaFailure : change.schemaError;
    if (actionsError === this.actionsFailure && schemaError === this.schemaFailure) return;
    this.actionsFailure = actionsError;
    this.schemaFailure = schemaError;
    const diagnostics = this.diagnostics();
    for (const listener of this.diagnosticListeners) listener(diagnostics);
  }

  private applySchema(db: Database, schema: string) {
    try {
      db.run(schema);
    } catch (error) {
      this.recordSchemaFailure(schema, error);
      return;
    }
    this.appliedSchema = schema;
    if (schema === this.requestedSchema) this.setDiagnostics({ schemaError: null });
    try {
      this.persist(db);
    } catch {
      // The schema only adds structure, and it runs again on the next open.
    }
  }

  private recordSchemaFailure(schema: string, error: unknown) {
    // A newer schema may already be on its way; its own result is the one to show.
    if (schema === this.requestedSchema) this.setDiagnostics({ schemaError: `schema.sql failed: ${describeError(error)}` });
  }

  /**
   * The methods a Capsule may call. It is the preview half of the SDK a
   * Deployment gets from the Kernel, and nothing wider: raw SQL is not on it,
   * because a published app has no way to send any. Server Actions reach the
   * database through `invokeAction`, which is the only door.
   */
  async handle(method: string, payload: unknown): Promise<unknown> {
    switch (method) {
      case "actions.invoke":
        return this.invokeAction(payload);
      case "auth.session":
        return this.getRuntimeSession();
      case "auth.signIn":
        this.setPreviewActor("alice");
        return this.getRuntimeSession();
      case "auth.signOut":
        this.setPreviewActor("guest");
        return this.getRuntimeSession();
      case "files.upload":
        return this.upload(payload);
      case "files.list":
        return this.visibleFiles();
      case "files.url":
        return this.fileUrl(payload);
      case "files.update":
        return this.updateFile(payload);
      case "files.delete":
        return this.deleteFile(payload);
      case "runtime.ready":
        return { ready: true };
      case "preview.error":
        return this.recordPreviewError(payload);
      default:
        throw new Error(`Unknown runtime method: ${method}`);
    }
  }

  async exportDatabase(): Promise<Uint8Array> {
    return (await this.dbPromise).export();
  }

  async listDatabaseBackups(): Promise<DatabaseBackup[]> {
    return this.readDatabaseBackups().map(({ database: _database, ...backup }) => backup);
  }

  async createDatabaseBackup(label = "Before migration", model?: { source: string; revision: string }): Promise<DatabaseBackup> {
    const bytes = await this.exportDatabase();
    const backup: StoredDatabaseBackup = {
      id: crypto.randomUUID(),
      label,
      createdAt: Date.now(),
      sizeBytes: bytes.byteLength,
      databaseModelSource: model?.source,
      schemaRevision: model?.revision,
      database: bytesToBase64(bytes),
    };
    const backups = [backup, ...this.readDatabaseBackups()].slice(0, localBackupLimit);
    localStorage.setItem(this.databaseBackupsKey, JSON.stringify(backups));
    const { database: _database, ...summary } = backup;
    return summary;
  }

  async restoreDatabaseBackup(backupId: string): Promise<DatabaseBackup> {
    const backup = this.readDatabaseBackups().find((candidate) => candidate.id === backupId);
    if (!backup) throw new Error("Backup not found.");

    const SQL = await loadSqlJs();
    const bytes = base64ToBytes(backup.database);
    assertRuntimeDatabaseWithinLimits(bytes);
    const restored = new SQL.Database(bytes);
    const integrity = restored.exec("PRAGMA integrity_check")[0]?.values[0]?.[0];
    if (integrity !== "ok") {
      restored.close();
      throw new Error("The backup failed SQLite's integrity check.");
    }

    const previous = await this.dbPromise;
    this.persist(restored);
    this.dbPromise = Promise.resolve(restored);
    previous.close();
    const { database: _database, ...summary } = backup;
    return summary;
  }

  async applyDatabaseMigration(input: ApplyDatabaseMigrationInput): Promise<{ backup: DatabaseBackup }> {
    if (input.statements.length === 0) throw new Error("The migration plan has no statements.");
    const SQL = await loadSqlJs();
    const current = await this.dbPromise;
    const clone = new SQL.Database(current.export());
    try {
      for (const statement of input.statements) clone.run(statement);
      const integrity = clone.exec("PRAGMA integrity_check")[0]?.values[0]?.[0];
      if (integrity !== "ok") throw new Error("The migrated database failed SQLite's integrity check.");
    } finally {
      clone.close();
    }

    const backup = await this.createDatabaseBackup(`Before migration · ${input.id.slice(0, 8)}`, {
      source: input.recoveryModelSource,
      revision: input.fromHash,
    });
    current.run("BEGIN IMMEDIATE");
    try {
      for (const statement of input.statements) current.run(statement);
      const integrity = current.exec("PRAGMA integrity_check")[0]?.values[0]?.[0];
      if (integrity !== "ok") throw new Error("The migrated database failed SQLite's integrity check.");
      current.run("COMMIT");
      this.persist(current);
      return { backup };
    } catch (error) {
      current.run("ROLLBACK");
      throw error;
    }
  }

  /**
   * Runtime failures reported by the running preview. Deduplicated by message
   * and bounded, so a capsule erroring in a render loop cannot flood the agent's
   * context with the same line thousands of times.
   */
  private recordPreviewError(payload: unknown): { recorded: boolean } {
    const input = (payload ?? {}) as { kind?: unknown; message?: unknown; stack?: unknown };
    const message = typeof input.message === "string" ? input.message.slice(0, 500) : "";
    if (!message) return { recorded: false };
    const kind = input.kind === "unhandledrejection" ? "unhandledrejection" : "error";
    const stack = typeof input.stack === "string" ? input.stack.slice(0, 1200) : "";
    const existing = this.previewFailures.find((failure) => failure.message === message);
    if (existing) {
      existing.count += 1;
      return { recorded: true };
    }
    this.previewFailures.push({ kind, message, stack, count: 1 });
    if (this.previewFailures.length > previewErrorLimit) this.previewFailures.shift();
    return { recorded: true };
  }

  /**
   * What the running preview threw, led by the Capsule files the host could not
   * load. Those come from `diagnostics()`, not the failure list, so a reconnect
   * clearing the list cannot hide that actions.json or schema.sql is broken.
   */
  previewErrors(): PreviewFailure[] {
    const fileFailures: PreviewFailure[] = [];
    if (this.actionsFailure) {
      // After a reload with a broken file there was never a valid program to fall back on.
      const fallback = this.actionProgram === emptyActionProgram
        ? "so the preview has no server actions until it is fixed"
        : "so the preview still runs the last valid one";
      fileFailures.push({ kind: "actions", message: `actions.json is invalid, ${fallback}: ${this.actionsFailure}`, stack: "", count: 1 });
    }
    if (this.schemaFailure) fileFailures.push({ kind: "schema", message: this.schemaFailure, stack: "", count: 1 });
    return [...fileFailures, ...this.previewFailures.map((failure) => ({ ...failure }))];
  }

  /** Called when a fresh document is about to run, so stale failures never mislead. */
  clearPreviewErrors() {
    this.previewFailures = [];
  }

  async inspectDatabase(): Promise<DatabaseTable[]> {
    const db = await this.dbPromise;
    const names = db.exec(
      "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )[0]?.values.flat().map(String) ?? [];

    return names.map((name) => {
      const escapedName = name.replaceAll('"', '""');
      const count = db.exec(`SELECT COUNT(*) FROM "${escapedName}"`)[0]?.values[0]?.[0];
      return {
        name,
        columns: this.tableColumns(db, escapedName),
        totalRows: Number(count ?? 0),
      };
    });
  }

  async runSql(sql: string): Promise<DatabaseRunResult> {
    if (!sql.trim()) throw new Error("Write a SQL statement first.");
    if (sql.length > 100_000) throw new Error("SQL script is too large.");

    const db = await this.dbPromise;
    const started = performance.now();
    const results = db.exec(sql);
    const result = results.at(-1);
    const columns = result?.columns ?? [];
    const rows = (result?.values ?? []).map((values) =>
      Object.fromEntries(columns.map((column, index) => [column, values[index]])),
    );
    const changes = db.getRowsModified();
    await this.persistOrRevert(db);

    return {
      columns,
      rows,
      changes,
      elapsedMs: performance.now() - started,
      statement: sql.trim().split(/\s+/, 1)[0]?.toUpperCase() ?? "SQL",
    };
  }

  async browseTable(
    tableName: string,
    options: { page?: number; pageSize?: number; sortColumn?: string; sortDirection?: "ASC" | "DESC" } = {},
  ): Promise<DatabasePage> {
    const db = await this.dbPromise;
    const columns = this.describeTable(db, tableName);
    const pageSize = Math.min(Math.max(options.pageSize ?? 50, 1), 100);
    const page = Math.max(options.page ?? 0, 0);
    const quotedTable = quoteIdentifier(tableName);
    const sortColumn = options.sortColumn && columns.some((column) => column.name === options.sortColumn)
      ? quoteIdentifier(options.sortColumn)
      : null;
    const order = sortColumn ? ` ORDER BY ${sortColumn} ${options.sortDirection === "DESC" ? "DESC" : "ASC"}` : "";
    const statement = db.prepare(
      `SELECT rowid AS __littlebox_rowid, * FROM ${quotedTable}${order} LIMIT ? OFFSET ?`,
    );
    const rows: DatabasePage["rows"] = [];
    try {
      statement.bind([pageSize, page * pageSize]);
      while (statement.step()) {
        const record = statement.getAsObject();
        const rowId = record.__littlebox_rowid ?? null;
        delete record.__littlebox_rowid;
        rows.push({ rowId, values: record });
      }
    } finally {
      statement.free();
    }
    const totalRows = Number(db.exec(`SELECT COUNT(*) FROM ${quotedTable}`)[0]?.values[0]?.[0] ?? 0);
    return { columns, rows, totalRows, page, pageSize };
  }

  async updateCell(tableName: string, rowId: SqlValue, columnName: string, value: SqlValue) {
    const db = await this.dbPromise;
    const columns = this.describeTable(db, tableName);
    if (!columns.some((column) => column.name === columnName)) throw new Error("Unknown column.");
    db.run(
      `UPDATE ${quoteIdentifier(tableName)} SET ${quoteIdentifier(columnName)} = ? WHERE rowid = ?`,
      [value, rowId],
    );
    await this.persistOrRevert(db);
  }

  async insertRow(tableName: string, values: Record<string, SqlValue>) {
    const db = await this.dbPromise;
    const columns = this.describeTable(db, tableName);
    const names = Object.keys(values);
    if (names.some((name) => !columns.some((column) => column.name === name))) throw new Error("Unknown column.");
    if (names.length === 0) {
      db.run(`INSERT INTO ${quoteIdentifier(tableName)} DEFAULT VALUES`);
    } else {
      db.run(
        `INSERT INTO ${quoteIdentifier(tableName)} (${names.map(quoteIdentifier).join(", ")}) VALUES (${names.map(() => "?").join(", ")})`,
        names.map((name) => values[name] ?? null),
      );
    }
    await this.persistOrRevert(db);
  }

  async deleteRow(tableName: string, rowId: SqlValue) {
    const db = await this.dbPromise;
    this.describeTable(db, tableName);
    db.run(`DELETE FROM ${quoteIdentifier(tableName)} WHERE rowid = ?`, [rowId]);
    await this.persistOrRevert(db);
  }

  getFiles(): StoredFile[] {
    return this.readFiles();
  }

  subscribeFiles(listener: (files: StoredFile[]) => void) {
    this.fileListeners.add(listener);
    listener(this.getFiles());
    const changedInAnotherTab = (event: StorageEvent) => {
      if (event.key === this.filesKey) listener(this.getFiles());
    };
    window.addEventListener("storage", changedInAnotherTab);
    return () => {
      this.fileListeners.delete(listener);
      window.removeEventListener("storage", changedInAnotherTab);
    };
  }

  private notifyFiles() {
    const files = this.getFiles();
    for (const listener of this.fileListeners) listener(files);
  }

  getSession(): RuntimeSession {
    return this.getRuntimeSession();
  }

  getPreviewActor(): PreviewActor {
    return this.previewActor;
  }

  subscribeSession(listener: (session: RuntimeSession) => void) {
    this.sessionListeners.add(listener);
    listener(this.getRuntimeSession());
    return () => {
      this.sessionListeners.delete(listener);
    };
  }

  setPreviewActor(actor: PreviewActor) {
    this.previewActor = actor;
    const session = this.getRuntimeSession();
    for (const listener of this.sessionListeners) listener(session);
  }

  reset() {
    localStorage.removeItem(this.databaseKey);
    localStorage.removeItem(this.filesKey);
    this.notifyFiles();
    this.dbPromise = this.openDatabase(true);
  }

  /** Resolves with a usable database even when schema.sql fails; the failure goes to `diagnostics()`. */
  private async openDatabase(ignoreStored = false): Promise<Database> {
    const SQL = await loadSqlJs();
    const stored = ignoreStored ? null : localStorage.getItem(this.databaseKey);
    const db = stored ? new SQL.Database(base64ToBytes(stored)) : new SQL.Database();
    this.appliedSchema = null;
    this.applySchema(db, this.requestedSchema);
    return db;
  }

  /**
   * The signed-out session has the Kernel's shape, so an app that checks it
   * behaves the same in the preview and once published. The "guest" actor is
   * the editor's name for that state and never reaches the app.
   */
  private getRuntimeSession(): RuntimeSession {
    if (this.previewActor === "guest") return { userId: "anonymous", name: "Anonymous", role: "anonymous" };
    return {
      userId: `${this.previewActor}_${this.appId}`,
      name: this.previewActor === "alice" ? "Alice" : "Bob",
      role: "member",
    };
  }

  private async query(payload: unknown) {
    const { sql, params } = this.parseSqlPayload(payload, "query");
    const db = await this.dbPromise;
    this.assertSingleRuntimeStatement(db, sql);
    const statement = db.prepare(sql);
    try {
      statement.bind(params);
      const rows: Record<string, SqlValue>[] = [];
      while (statement.step()) {
        rows.push(statement.getAsObject());
        if (rows.length > runtimeLimits.resultRows) {
          throw new Error(queryResultTooLarge);
        }
      }
      assertRuntimeResultWithinLimits(rows);
      return rows;
    } finally {
      statement.free();
    }
  }

  private async invokeAction(payload: unknown) {
    if (!payload || typeof payload !== "object" || !("name" in payload)) throw new Error("A server action name is required.");
    const request = payload as { name: unknown; input?: unknown };
    // The runtime endpoint checks its arguments before it looks up the action.
    if (typeof request.name !== "string") throw new Error("Expected a string.");
    if (request.input !== undefined && (!request.input || typeof request.input !== "object" || Array.isArray(request.input))) {
      throw new Error("Arguments must be an object.");
    }
    const declaration = this.actionProgram.actions[request.name];
    if (!declaration && this.actionsFailure) {
      throw new Error(`Server action not declared. actions.json is invalid: ${this.actionsFailure}`);
    }
    // The preview has no visibility, so it refuses what every Deployment refuses:
    // an action whose signed or callers-limited route is its only door.
    if (declaration?.http && routeCallers(declaration.http, "public")) {
      throw new Error(`Action ${declaration.name} answers only on its HTTP route, ${declaration.http.method} ${declaration.http.path}.`);
    }
    // A call's then mutation runs only after its call answers, on a Deployment too.
    const call = declaration ? thenOf(this.actionProgram.actions, declaration.name) : null;
    if (call) throw new Error(`Action ${declaration!.name} runs only after the call ${call} answers.`);
    if (declaration && declaration.access !== "anonymous" && this.previewActor === "guest") {
      throw new Error("authentication-required");
    }
    const { action, params } = resolveServerAction(this.actionProgram, String(request.name), request.input, {
      actorId: this.previewActor === "guest" ? null : `${this.previewActor}_${this.appId}`,
      actorRole: this.previewActor === "guest" ? null : this.getRuntimeSession().role,
    });
    // The preview keeps no Usage store. A signal action runs no SQL and answers
    // as the Kernel does, which records the Signal on a Deployment.
    if (action.kind === "signal") return { recorded: true };
    // The preview sends no call: a Deployment's Kernel does, from its own process.
    if (action.kind === "call") return { callId: `preview_${crypto.randomUUID()}`, queued: true };
    if (action.kind === "query") return this.query({ sql: action.statement, params });
    const result = await this.execute({ sql: action.statement, params });
    // A guarded mutation that changed no row is refused with its guard, as on a Deployment.
    if (action.guard && result.changes === 0) throw new Error(action.guard);
    return result;
  }

  private async execute(payload: unknown) {
    const { sql, params } = this.parseSqlPayload(payload, "execute");
    const db = await this.dbPromise;
    this.assertSingleRuntimeStatement(db, sql);
    db.run("SAVEPOINT littlebox_runtime_write");
    let released = false;
    try {
      db.run(sql, params);
      const changes = db.getRowsModified();
      const lastIdResult = db.exec("SELECT last_insert_rowid() AS id");
      const lastInsertId = lastIdResult[0]?.values[0]?.[0] ?? null;
      assertRuntimeDatabaseWithinLimits(this.databaseAllocatedBytes(db));
      db.run("RELEASE littlebox_runtime_write");
      released = true;
      await this.persistOrRevert(db);
      return { changes, lastInsertId };
    } catch (error) {
      if (!released) {
        db.run("ROLLBACK TO littlebox_runtime_write");
        db.run("RELEASE littlebox_runtime_write");
      }
      throw error;
    }
  }

  private parseSqlPayload(payload: unknown, operation: "query" | "execute"): { sql: string; params: SqlValue[] } {
    if (!payload || typeof payload !== "object" || !("sql" in payload)) {
      throw new Error("A SQL string is required.");
    }
    const sql = String(payload.sql);
    assertRuntimeSql(operation, sql);
    const params = validateRuntimeParams("params" in payload ? payload.params : []);
    return { sql, params };
  }

  private assertSingleRuntimeStatement(db: Database, sql: string) {
    let statements = 0;
    for (const statement of db.iterateStatements(sql)) {
      statements += 1;
      statement.free();
    }
    if (statements !== 1) throw new Error("Runtime calls must contain exactly one SQL statement.");
  }

  private databaseAllocatedBytes(db: Database): number {
    const pageCount = Number(db.exec("PRAGMA page_count")[0]?.values[0]?.[0] ?? 0);
    const pageSize = Number(db.exec("PRAGMA page_size")[0]?.values[0]?.[0] ?? 0);
    return pageCount * pageSize;
  }

  /**
   * Persist a change already made in memory, or undo it. sql.js can only export
   * by closing and reopening the database, which ends any open savepoint, so the
   * write cannot be held open across the save. If the save fails (a full
   * localStorage quota, say), the in-memory database is replaced by the last
   * stored copy: a write the app was told failed must not be visible either.
   */
  private async persistOrRevert(db: Database) {
    try {
      this.persist(db);
    } catch (error) {
      const SQL = await loadSqlJs();
      const stored = localStorage.getItem(this.databaseKey);
      const restored = stored ? new SQL.Database(base64ToBytes(stored)) : new SQL.Database();
      // The stored copy can predate a schema change whose own save failed.
      this.applySchema(restored, this.requestedSchema);
      this.dbPromise = Promise.resolve(restored);
      db.close();
      throw error;
    }
  }

  private persist(db: Database, exported?: Uint8Array) {
    const bytes = exported ?? db.export();
    assertRuntimeDatabaseWithinLimits(bytes);
    localStorage.setItem(this.databaseKey, bytesToBase64(bytes));
  }

  private tableColumns(db: Database, escapedName: string): string[] {
    return db.exec(`PRAGMA table_info("${escapedName}")`)[0]?.values.map((row) => String(row[1])) ?? [];
  }

  private describeTable(db: Database, tableName: string): DatabaseColumn[] {
    const exists = db.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?");
    try {
      exists.bind([tableName]);
      if (!exists.step()) throw new Error(`Table not found: ${tableName}`);
    } finally {
      exists.free();
    }
    const escapedName = tableName.replaceAll('"', '""');
    return (db.exec(`PRAGMA table_info("${escapedName}")`)[0]?.values ?? []).map((row) => ({
      name: String(row[1]),
      type: String(row[2] || "ANY"),
      notNull: Boolean(row[3]),
      defaultValue: row[4] ?? null,
      primaryKey: Boolean(row[5]),
    }));
  }

  private upload(payload: unknown): StoredFile {
    if (this.previewActor === "guest") throw new Error("authentication-required");
    if (!payload || typeof payload !== "object") throw new Error("Invalid file.");
    const candidate = payload as Record<string, unknown>;
    const url = String(candidate.dataUrl ?? "");
    const size = Number(candidate.size ?? 0);
    if (!url.startsWith("data:")) throw new Error("Only local data URLs are supported in this prototype.");
    const visibility = candidate.visibility ?? "private";
    if (visibility !== "private" && visibility !== "public") throw new Error(badFileVisibility);
    // The Kernel's limit and words for a transfer: 1 byte to 2 MiB.
    if (!(size >= 1 && size <= 2 * 1024 * 1024)) throw new Error("Transfer size is outside the limit.");
    const file: StoredFile = {
      id: crypto.randomUUID(),
      name: String(candidate.name ?? "file"),
      type: String(candidate.type ?? "application/octet-stream"),
      size,
      url,
      createdAt: Date.now(),
      visibility,
      ownerId: `${this.previewActor}_${this.appId}`,
    };
    const files = this.readFiles();
    localStorage.setItem(this.filesKey, JSON.stringify([file, ...files]));
    this.notifyFiles();
    return file;
  }

  private visibleFiles(): StoredFile[] {
    const session = this.getRuntimeSession();
    const actor = session.role === "anonymous" ? null : session.userId;
    return this.readFiles().flatMap((file) => {
      const visibility = file.visibility === "public" ? "public" : "private";
      const readable = visibility === "public" || (actor !== null && file.ownerId === actor);
      if (!readable) return [];
      if (file.ownerId !== actor) {
        const rest = { ...file };
        delete rest.ownerId;
        return [{ ...rest, visibility }];
      }
      return [{ ...file, visibility }];
    });
  }

  /** The preview has no file path to serve, so files.url answers the stored data URL. */
  private fileUrl(payload: unknown): string {
    const id = requireFileId(payload);
    const file = this.visibleFiles().find((candidate) => candidate.id === id);
    if (!file) throw new Error("File not found.");
    return file.url;
  }

  /** The uploader alone changes a file, with the Kernel's refusals in its order. */
  private ownedFile(payload: unknown): { files: StoredFile[]; index: number } {
    if (this.previewActor === "guest") throw new Error("authentication-required");
    const id = requireFileId(payload);
    if (!this.visibleFiles().some((file) => file.id === id)) throw new Error("File not found.");
    const files = this.readFiles();
    const index = files.findIndex((file) => file.id === id);
    if (files[index].ownerId !== this.getRuntimeSession().userId) throw new Error(notFileUploader);
    return { files, index };
  }

  private updateFile(payload: unknown) {
    const visibility = (payload as { visibility?: unknown } | null)?.visibility;
    if (visibility !== "private" && visibility !== "public") throw new Error(badFileVisibility);
    const { files, index } = this.ownedFile(payload);
    files[index] = { ...files[index], visibility };
    localStorage.setItem(this.filesKey, JSON.stringify(files));
    this.notifyFiles();
    const { url: _url, ...file } = files[index];
    return file;
  }

  private deleteFile(payload: unknown) {
    const { files, index } = this.ownedFile(payload);
    const [removed] = files.splice(index, 1);
    localStorage.setItem(this.filesKey, JSON.stringify(files));
    this.notifyFiles();
    return { id: removed.id, deleted: true };
  }

  private readFiles(): StoredFile[] {
    try {
      return JSON.parse(localStorage.getItem(this.filesKey) ?? "[]") as StoredFile[];
    } catch {
      return [];
    }
  }

  private readDatabaseBackups(): StoredDatabaseBackup[] {
    try {
      const parsed = JSON.parse(localStorage.getItem(this.databaseBackupsKey) ?? "[]") as unknown;
      if (!Array.isArray(parsed)) return [];
      return parsed.filter((backup): backup is StoredDatabaseBackup => (
        typeof backup === "object" && backup !== null
        && typeof backup.id === "string"
        && typeof backup.label === "string"
        && typeof backup.createdAt === "number"
        && typeof backup.sizeBytes === "number"
        && typeof backup.database === "string"
        && (backup.databaseModelSource === undefined || typeof backup.databaseModelSource === "string")
        && (backup.schemaRevision === undefined || typeof backup.schemaRevision === "string")
      ));
    } catch {
      return [];
    }
  }

  private get databaseKey() {
    return `littlebox:database:${this.appId}`;
  }

  private get filesKey() {
    return `littlebox:files:${this.appId}`;
  }

  private get databaseBackupsKey() {
    return `littlebox:database-backups:${this.appId}`;
  }
}

const badFileVisibility = "File visibility must be private or public.";
const notFileUploader = "Only the person who uploaded this file may change or delete it.";

function requireFileId(payload: unknown): string {
  const id = (payload as { fileId?: unknown } | null)?.fileId;
  if (typeof id !== "string" || !id) throw new Error("A file ID is required.");
  return id;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function quoteIdentifier(value: string): string {
  return `"${value.replaceAll('"', '""')}"`;
}
