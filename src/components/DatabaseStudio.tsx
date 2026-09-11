import {
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  Clock3,
  Code2,
  Database,
  Download,
  FileCode2,
  History,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Table2,
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { SqlValue } from "sql.js";
import type {
  DatabasePage,
  DatabaseBackup,
  DatabaseRunResult,
  DatabaseTable,
  DatabaseAdminHost,
} from "../runtime/host";
import { planDatabaseChange } from "../database-lifecycle/client";
import type { DatabaseMigrationPlan } from "../database-lifecycle/contracts";
import type { DrizzleDatabaseModel } from "../lib/types";
import { SqlEditor } from "./SqlEditor";

type StudioTab = "data" | "sql" | "schema" | "backups";
type SortState = { column: string; direction: "ASC" | "DESC" } | null;

type Props = {
  host: DatabaseAdminHost;
  databaseModel: DrizzleDatabaseModel;
  onEditSchema: () => void;
  onDatabaseModelAccepted: (source: string, revision: string) => void | Promise<void>;
  onDatabaseModelRestored: (source: string, revision: string) => void | Promise<void>;
  onExportDatabase: () => void;
  target?: "local" | "deployed";
  deployedLabel?: string;
  onTargetChange?: (target: "local" | "deployed") => void;
};

export function DatabaseStudio({ host, databaseModel, onEditSchema, onDatabaseModelAccepted, onDatabaseModelRestored, onExportDatabase, target = "local", deployedLabel, onTargetChange }: Props) {
  const [tab, setTab] = useState<StudioTab>("data");
  const [tables, setTables] = useState<DatabaseTable[]>([]);
  const [selectedTable, setSelectedTable] = useState("");
  const [refreshVersion, setRefreshVersion] = useState(0);
  const [tableError, setTableError] = useState("");

  const refreshTables = async (preferred?: string) => {
    try {
      const next = await host.inspectDatabase();
      setTables(next);
      setSelectedTable((current) => {
        const candidate = preferred ?? current;
        return next.some((table) => table.name === candidate) ? candidate : next[0]?.name ?? "";
      });
      setTableError("");
    } catch (cause) {
      setTableError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  useEffect(() => { void refreshTables(); }, [host, refreshVersion]);

  const mutated = () => setRefreshVersion((current) => current + 1);

  return (
    <section className="database-studio" aria-label="Database panel">
      {/* No title here: the tab that opened this panel already names it, and the
          group toolbar carries Export. What stays is what the tab cannot say —
          which view of the database, and which database. */}
      {tableError && <div className="backup-error" role="alert">Could not refresh database tables: {tableError}</div>}
      <header className="database-studio-header">
        <nav className="database-tabs" aria-label="Database views">
          <button className={tab === "data" ? "active" : ""} onClick={() => setTab("data")}><Table2 size={13} /> Data</button>
          <button className={tab === "sql" ? "active" : ""} onClick={() => setTab("sql")}><Code2 size={13} /> SQL</button>
          <button className={tab === "schema" ? "active" : ""} onClick={() => setTab("schema")}><FileCode2 size={13} /> Schema</button>
          <button className={tab === "backups" ? "active" : ""} onClick={() => setTab("backups")}><History size={13} /> Backups</button>
        </nav>
        {onTargetChange && (
          <select className="database-target" aria-label="Database target" value={target} onChange={(event) => onTargetChange(event.target.value as "local" | "deployed")}>
            <option value="local">Local preview</option>
            <option value="deployed">Deployed · {deployedLabel}</option>
          </select>
        )}
      </header>

      {tab === "data" && (
        <DataBrowser
          host={host}
          tables={tables}
          selectedTable={selectedTable}
          onSelectTable={setSelectedTable}
          refreshVersion={refreshVersion}
          onMutated={mutated}
        />
      )}
      {tab === "sql" && <SqlConsole host={host} tables={tables} onMutated={mutated} />}
      {tab === "schema" && <SchemaView host={host} databaseModel={databaseModel} onEdit={onEditSchema} onApplied={async (source, revision) => { await onDatabaseModelAccepted(source, revision); mutated(); }} />}
      {tab === "backups" && <BackupsView databaseModel={databaseModel} host={host} tables={tables} onExport={onExportDatabase} target={target} onRestored={async (backup) => { if (backup.databaseModelSource) await onDatabaseModelRestored(backup.databaseModelSource, backup.schemaRevision ?? "restored"); mutated(); }} />}

      <footer className="database-statusbar">
        <span><i /> {target === "deployed" ? "SQLite Kernel" : "SQLite WASM"}</span>
        <span>{target === "deployed" ? "isolated deployed database" : "local persistent database"}</span>
        <span>{tables.length} table{tables.length === 1 ? "" : "s"}</span>
        <span className="statusbar-right">click a cell to edit · changes save instantly</span>
      </footer>
    </section>
  );
}

function DataBrowser({ host, tables, selectedTable, onSelectTable, refreshVersion, onMutated }: {
  host: DatabaseAdminHost;
  tables: DatabaseTable[];
  selectedTable: string;
  onSelectTable: (table: string) => void;
  refreshVersion: number;
  onMutated: () => void;
}) {
  return (
    <div className="data-browser">
      <aside className="data-table-sidebar">
        <div className="data-sidebar-label"><span>TABLES</span><span>{tables.length}</span></div>
        <div className="data-sidebar-list">
          {tables.map((table) => (
            <button key={table.name} className={selectedTable === table.name ? "active" : ""} onClick={() => onSelectTable(table.name)}>
              <Table2 size={12} /><span><strong>{table.name}</strong><small>{table.columns.length} columns</small></span><em>{table.totalRows}</em>
            </button>
          ))}
        </div>
        <div className="data-sidebar-note"><Database size={13} /><p>Every mini-app gets an isolated SQLite database.</p></div>
      </aside>
      <div className="data-grid-region">
        {selectedTable ? (
          <EditableTable
            key={selectedTable}
            host={host}
            tableName={selectedTable}
            refreshVersion={refreshVersion}
            onMutated={onMutated}
          />
        ) : <StudioEmpty title="No tables yet" detail="Edit schema.sql and run the app to create your first table." />}
      </div>
    </div>
  );
}

/**
 * The grid keeps its own sort and page, so a mutation refreshes it through a
 * dependency rather than through the key: remounting on every edited cell threw
 * away the order and the page the owner had chosen, with no sign that it had.
 */
function EditableTable({ host, tableName, refreshVersion, onMutated }: {
  host: DatabaseAdminHost;
  tableName: string;
  /** Bumped by any write, here or in the SQL console: the page is re-read, not rebuilt. */
  refreshVersion: number;
  onMutated: () => void;
}) {
  const [data, setData] = useState<DatabasePage | null>(null);
  const [page, setPage] = useState(0);
  const [sort, setSort] = useState<SortState>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [editing, setEditing] = useState<{ rowId: SqlValue; column: string; value: string } | null>(null);
  const [insertOpen, setInsertOpen] = useState(false);
  const [insertValues, setInsertValues] = useState<Record<string, string>>({});
  const [deleteRowId, setDeleteRowId] = useState<SqlValue | null>(null);

  const load = async () => {
    setLoading(true);
    setError("");
    try {
      const next = await host.browseTable(tableName, {
        page,
        pageSize: 50,
        sortColumn: sort?.column,
        sortDirection: sort?.direction,
      });
      // Deleting the last rows of the last page leaves the owner past the end.
      const lastPage = Math.max(0, Math.ceil(next.totalRows / 50) - 1);
      if (page > lastPage) {
        setPage(lastPage);
        return;
      }
      setData(next);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { void load(); }, [host, tableName, page, sort, refreshVersion]);

  const toggleSort = (column: string) => {
    setSort((current) => {
      if (!current || current.column !== column) return { column, direction: "ASC" };
      if (current.direction === "ASC") return { column, direction: "DESC" };
      return null;
    });
    setPage(0);
  };

  const commitEdit = async () => {
    if (!editing) return;
    try {
      await host.updateCell(tableName, editing.rowId, editing.column, editing.value === "" ? null : editing.value);
      setEditing(null);
      onMutated();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const openInsert = () => {
    const values = Object.fromEntries((data?.columns ?? []).filter((column) => !column.primaryKey).map((column) => [column.name, ""]));
    setInsertValues(values);
    setInsertOpen(true);
  };

  const commitInsert = async () => {
    if (!data) return;
    try {
      const values = Object.fromEntries(
        data.columns
          .filter((column) => !column.primaryKey)
          .flatMap((column) => {
            const value = insertValues[column.name] ?? "";
            if (value === "" && column.defaultValue !== null) return [];
            return [[column.name, value === "" ? null : value]];
          }),
      );
      await host.insertRow(
        tableName,
        values,
      );
      setInsertOpen(false);
      setPage(0);
      onMutated();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const commitDelete = async () => {
    if (deleteRowId === null) return;
    try {
      await host.deleteRow(tableName, deleteRowId);
      setDeleteRowId(null);
      onMutated();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const pages = Math.max(1, Math.ceil((data?.totalRows ?? 0) / 50));

  return (
    <section className="editable-table" aria-label={`${tableName} table editor`}>
      <div className="table-toolbar">
        <div><p>TABLE</p><h3>{tableName}</h3><span>{data?.totalRows ?? 0} rows</span></div>
        <div className="table-toolbar-actions">
          <button onClick={() => void load()}><RefreshCw size={12} /> Refresh</button>
          <button className="insert-row-button" onClick={openInsert}><Plus size={12} /> Insert row</button>
        </div>
      </div>

      {error && <div className="table-error"><span>{error}</span><button onClick={() => setError("")}><X size={12} /></button></div>}

      {insertOpen && data && (
        <div className="insert-row-form">
          <div className="insert-title"><div><p>NEW ROW</p><strong>Insert into {tableName}</strong></div><button onClick={() => setInsertOpen(false)}><X size={14} /></button></div>
          <div className="insert-fields">
            {data.columns.filter((column) => !column.primaryKey).map((column) => (
              <label key={column.name}><span>{column.name}<em>{column.type}</em></span><input value={insertValues[column.name] ?? ""} placeholder={column.defaultValue !== null ? `default: ${String(column.defaultValue)}` : column.notNull ? "required" : "null"} onChange={(event) => setInsertValues((current) => ({ ...current, [column.name]: event.target.value }))} /></label>
            ))}
          </div>
          <div className="insert-actions"><button onClick={() => setInsertOpen(false)}>Cancel</button><button className="primary" onClick={() => void commitInsert()}><Plus size={12} /> Insert row</button></div>
        </div>
      )}

      <div className="editable-grid-wrap">
        {loading ? <StudioEmpty title="Reading SQLite…" detail="Loading table rows and column metadata." loading /> : !data || data.rows.length === 0 ? <StudioEmpty title={`No rows in ${tableName}`} detail="Insert the first row to start using this table." /> : (
          <table className="editable-grid">
            <thead><tr><th className="grid-row-number">#</th>{data.columns.map((column) => (
              <th key={column.name}><button onClick={() => toggleSort(column.name)}><span>{column.name}<small>{column.type}</small></span>{sort?.column === column.name ? sort.direction === "ASC" ? <ChevronUp size={12} /> : <ChevronDown size={12} /> : null}</button></th>
            ))}<th className="grid-actions-heading" /></tr></thead>
            <tbody>{data.rows.map((row, rowIndex) => (
              <tr key={String(row.rowId)}>
                <td className="grid-row-number">{page * 50 + rowIndex + 1}</td>
                {data.columns.map((column) => {
                  const value = row.values[column.name];
                  const isEditing = editing?.rowId === row.rowId && editing.column === column.name;
                  return <td key={column.name} className={isEditing ? "editing" : ""}>{isEditing ? (
                    <div className="cell-editor"><input autoFocus value={editing.value} onChange={(event) => setEditing({ ...editing, value: event.target.value })} onKeyDown={(event) => { if (event.key === "Enter") void commitEdit(); if (event.key === "Escape") setEditing(null); }} /><button onClick={() => void commitEdit()}><Check size={11} /></button></div>
                  ) : (
                    <button className={`cell-value ${value === null ? "is-null" : ""}`} onClick={() => setEditing({ rowId: row.rowId, column: column.name, value: value === null ? "" : String(value) })}>{value === null ? "null" : formatCellText(value)}<Pencil size={9} /></button>
                  )}</td>;
                })}
                <td className="grid-row-actions">{deleteRowId === row.rowId ? <div><button className="confirm-delete" onClick={() => void commitDelete()}>Delete</button><button onClick={() => setDeleteRowId(null)}><X size={11} /></button></div> : <button aria-label={`Delete row ${rowIndex + 1}`} onClick={() => setDeleteRowId(row.rowId)}><Trash2 size={12} /></button>}</td>
              </tr>
            ))}</tbody>
          </table>
        )}
      </div>

      <footer className="table-pagination">
        <span>Page {page + 1} of {pages}</span>
        <div><button disabled={page === 0} onClick={() => setPage((current) => current - 1)}><ChevronLeft size={13} /> Prev</button><button disabled={page >= pages - 1} onClick={() => setPage((current) => current + 1)}>Next <ChevronRight size={13} /></button></div>
      </footer>
    </section>
  );
}

function SqlConsole({ host, tables, onMutated }: {
  host: DatabaseAdminHost;
  tables: DatabaseTable[];
  onMutated: () => void;
}) {
  const [query, setQuery] = useState(() => tables[0] ? `SELECT * FROM "${tables[0].name}" LIMIT 100;` : "SELECT sqlite_version() AS version;");
  const [result, setResult] = useState<DatabaseRunResult | null>(null);
  const [error, setError] = useState("");
  const [running, setRunning] = useState(false);
  const [history, setHistory] = useState<Array<{ sql: string; elapsedMs: number; ok: boolean }>>([]);
  const [showHistory, setShowHistory] = useState(false);
  const editorSchema = useMemo(() => Object.fromEntries(tables.map((table) => [table.name, table.columns])), [tables]);

  const run = async () => {
    if (!query.trim() || running) return;
    setRunning(true);
    setError("");
    const started = performance.now();
    try {
      const next = await host.runSql(query);
      setResult(next);
      setHistory((current) => [{ sql: query, elapsedMs: next.elapsedMs, ok: true }, ...current].slice(0, 30));
      if (next.changes > 0) onMutated();
    } catch (cause) {
      setResult(null);
      setError(cause instanceof Error ? cause.message : String(cause));
      setHistory((current) => [{ sql: query, elapsedMs: performance.now() - started, ok: false }, ...current].slice(0, 30));
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="standalone-sql">
      <div className="sql-page-heading"><div><p>SQL CONSOLE</p><h3>Query the raw database</h3><span>Use SQL for advanced inspection. Schema changes should stay in schema.sql.</span></div><button onClick={() => void run()} disabled={running}><Play size={12} fill="currentColor" /> {running ? "Running…" : "Run query"}</button></div>
      <div className="sql-surface">
        <div className="sql-query-title"><span><i /> Query 1</span><small>⌘↵ to run</small></div>
        <div className="standalone-editor"><SqlEditor value={query} schema={editorSchema} onChange={setQuery} onExecute={() => void run()} /></div>
        <div className="sql-result-tabs"><button className={!showHistory ? "active" : ""} onClick={() => setShowHistory(false)}>Results</button><button className={showHistory ? "active" : ""} onClick={() => setShowHistory(true)}>History <span>{history.length}</span></button>{result && !showHistory && <small>{result.rows.length} rows · {result.elapsedMs.toFixed(1)} ms</small>}</div>
        <div className="standalone-results">{showHistory ? <QueryHistory history={history} onSelect={(sql) => { setQuery(sql); setShowHistory(false); }} /> : <QueryResult result={result} error={error} />}</div>
      </div>
    </div>
  );
}

function QueryResult({ result, error }: { result: DatabaseRunResult | null; error: string }) {
  if (error) return <div className="sql-error"><strong>SQLite error</strong><code>{error}</code></div>;
  if (!result) return <StudioEmpty title="Ready for a query" detail="Write SQL above and press ⌘↵ to execute it." />;
  if (result.columns.length === 0) return <div className="sql-success"><strong>Query executed</strong><span>{result.changes} row{result.changes === 1 ? "" : "s"} affected</span></div>;
  return <table className="data-table"><thead><tr><th className="row-index">#</th>{result.columns.map((column) => <th key={column}>{column}</th>)}</tr></thead><tbody>{result.rows.map((row, index) => <tr key={index}><td className="row-index">{index + 1}</td>{result.columns.map((column) => <td key={column}>{formatCellText(row[column])}</td>)}</tr>)}</tbody></table>;
}

function QueryHistory({ history, onSelect }: { history: Array<{ sql: string; elapsedMs: number; ok: boolean }>; onSelect: (sql: string) => void }) {
  if (history.length === 0) return <StudioEmpty title="No query history" detail="Queries executed in this session will appear here." />;
  return <div className="query-history">{history.map((item, index) => <button key={`${item.sql}-${index}`} onClick={() => onSelect(item.sql)}><i className={item.ok ? "ok" : "error"} /><code>{item.sql}</code><span>{item.elapsedMs.toFixed(1)} ms</span></button>)}</div>;
}

function SchemaView({ host, databaseModel, onEdit, onApplied }: {
  host: DatabaseAdminHost;
  databaseModel: DrizzleDatabaseModel;
  onEdit: () => void;
  onApplied: (source: string, revision: string) => void | Promise<void>;
}) {
  const [plan, setPlan] = useState<DatabaseMigrationPlan | null>(null);
  const [planning, setPlanning] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");

  useEffect(() => {
    setPlan(null);
    setError("");
    if (databaseModel.source === databaseModel.acceptedSource) {
      setPlanning(false);
      return;
    }
    setMessage("");
    setPlanning(true);
    const controller = new AbortController();
    const timeout = window.setTimeout(() => {
      void planDatabaseChange({
        acceptedSource: databaseModel.acceptedSource,
        candidateSource: databaseModel.source,
      }, { signal: controller.signal }).then((next) => {
        if (!controller.signal.aborted) setPlan(next);
      }).catch((cause) => {
        if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
      }).finally(() => {
        if (!controller.signal.aborted) setPlanning(false);
      });
    }, 400);
    return () => {
      window.clearTimeout(timeout);
      controller.abort();
    };
  }, [host, databaseModel.source, databaseModel.acceptedSource]);

  const apply = async () => {
    if (!plan || !host.applyDatabaseMigration) return;
    setApplying(true);
    setError("");
    setMessage("");
    try {
      await host.applyDatabaseMigration({ ...plan, recoveryModelSource: databaseModel.acceptedSource });
      await onApplied(databaseModel.source, plan.toHash);
      setMessage("Migration applied · recovery point created");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setApplying(false);
    }
  };

  const inSync = databaseModel.source === databaseModel.acceptedSource;
  const lifecycleStatus = message
    || (applying ? "Applying migration · creating recovery point"
      : planning ? "Changes detected · generating Drizzle migration"
        : plan ? "Migration ready · review generated SQL"
          : error ? "Migration planning failed"
            : inSync ? "Database model in sync" : "Changes detected · waiting to generate migration");

  return (
    <div className="studio-document-view schema-model-view">
      <header>
        <div><p>DRIZZLE DATABASE MODEL</p><h3>schema.ts is the source of truth</h3><span>Littlebox generates and reviews SQL; the model remains editable.</span></div>
        <div className="schema-actions"><button onClick={onEdit}><Pencil size={12} /> Edit schema.ts</button></div>
      </header>
      <pre><code>{databaseModel.source}</code></pre>
      <div className={`schema-lifecycle-status ${error ? "is-error" : plan ? "is-ready" : inSync ? "is-synced" : ""}`} role="status">{planning && <RefreshCw className="spin" size={12} />}{!planning && plan && <Check size={12} />}{lifecycleStatus}</div>
      {error && <div className="backup-error" role="alert">{error}</div>}
      {plan ? (
        <section className="migration-plan" aria-label="Migration plan">
          <header><div><p>DRIZZLE KIT PLAN</p><h4>Migration ready</h4><small>{plan.statements.length} statement{plan.statements.length === 1 ? "" : "s"} · {plan.id.slice(0, 8)}</small></div>{plan.destructive && <strong>OWNER APPROVAL REQUIRED</strong>}</header>
          <pre><code>{plan.sql}</code></pre>
          <footer><span>A backup will be created immediately before apply.</span><button onClick={() => void apply()} disabled={applying || !host.applyDatabaseMigration}>{applying ? "Applying…" : plan.destructive ? "Approve and apply migration" : "Apply migration"}</button></footer>
        </section>
      ) : <aside><FileCode2 size={18} /><div><strong>Managed Drizzle lifecycle</strong><p>Edit the model, generate a deterministic SQL diff, then apply it with an automatic recovery point.</p></div></aside>}
    </div>
  );
}

function BackupsView({ host, tables, onExport, target, onRestored, databaseModel }: {
  databaseModel: DrizzleDatabaseModel;
  host: DatabaseAdminHost;
  tables: DatabaseTable[];
  onExport: () => void;
  target: "local" | "deployed";
  onRestored: (backup: DatabaseBackup) => void | Promise<void>;
}) {
  const rowCount = tables.reduce((total, table) => total + table.totalRows, 0);
  const supportsRestore = Boolean(host.listDatabaseBackups && host.createDatabaseBackup && host.restoreDatabaseBackup);
  const [backups, setBackups] = useState<DatabaseBackup[]>([]);
  const [restoreId, setRestoreId] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");

  const loadBackups = async () => {
    if (!host.listDatabaseBackups) return;
    setBackups(await host.listDatabaseBackups());
  };

  useEffect(() => { void loadBackups().catch((cause) => setError(String(cause))); }, [host]);

  const createBackup = async () => {
    if (!host.createDatabaseBackup) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await host.createDatabaseBackup("Manual backup", { source: databaseModel.acceptedSource, revision: databaseModel.revision });
      await loadBackups();
      setMessage("Backup created");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const restoreBackup = async () => {
    if (!host.restoreDatabaseBackup || !restoreId) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      const backup = await host.restoreDatabaseBackup(restoreId);
      setRestoreId("");
      await onRestored(backup);
      setMessage("Backup restored");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="studio-document-view backups-view">
      <header>
        <div><p>BACKUPS</p><h3>Your database is portable</h3><span>Create a restore point before changing its structure.</span></div>
        {supportsRestore
          ? <button onClick={() => void createBackup()} disabled={busy}><History size={12} /> Create backup</button>
          : <button onClick={onExport}><Download size={12} /> Download snapshot</button>}
      </header>
      <div className="backup-card">
        <span className="backup-icon"><Clock3 size={24} /></span>
        <div><p>CURRENT {target === "deployed" ? "DEPLOYED" : "LOCAL"} SNAPSHOT</p><h4>database.sqlite</h4><small>{tables.length} tables · {rowCount} rows · ready to export</small></div>
        <button onClick={onExport}><Download size={13} /> Export</button>
      </div>
      {message && <div className="backup-message" role="status">{message}</div>}
      {error && <div className="backup-error" role="alert">{error}</div>}
      {supportsRestore ? (
        backups.length > 0 ? (
          <ul className="backup-list" aria-label="Database backups">
            {backups.map((backup) => (
              <li key={backup.id}>
                <div><strong>{backup.label}</strong><small>{new Date(backup.createdAt).toLocaleString()} · {formatBytes(backup.sizeBytes)}</small></div>
                {restoreId === backup.id ? (
                  <div className="backup-confirm"><span>Replace the current database?</span><button onClick={() => void restoreBackup()} disabled={busy}>Confirm restore</button><button onClick={() => setRestoreId("")} disabled={busy}>Cancel</button></div>
                ) : <button onClick={() => setRestoreId(backup.id)} disabled={busy}>Restore</button>}
              </li>
            ))}
          </ul>
        ) : <div className="future-backups"><History size={20} /><div><strong>No restore points yet</strong><p>Create a backup before running a migration. Local previews keep five snapshots. Published backups stay on the kernel volume.</p></div></div>
      ) : <div className="future-backups"><History size={20} /><div><strong>Managed restore is not available for this target yet</strong><p>Download a consistent SQLite snapshot before changing the deployed database.</p></div></div>}
    </div>
  );
}

function StudioEmpty({ title, detail, loading = false }: { title: string; detail: string; loading?: boolean }) {
  return <div className="studio-empty">{loading ? <RefreshCw className="spin" size={22} /> : <Table2 size={22} />}<strong>{title}</strong><p>{detail}</p></div>;
}

function formatCellText(value: unknown): string {
  if (value === null) return "null";
  if (value instanceof Uint8Array) return `[blob · ${value.byteLength} bytes]`;
  if (typeof value === "number" && value > 1_000_000_000_000) return new Date(value).toLocaleString();
  return String(value);
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  return `${(value / 1024).toFixed(1)} KiB`;
}
