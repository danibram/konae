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
import { Badge, Button, Dialog, EmptyState, LoadingBlock, Segmented, TabBar } from "./ui";
import { SqlEditor } from "./SqlEditor";
import { classifySql } from "./sql-statements";

type StudioTab = "data" | "sql" | "schema" | "backups";
type SortState = { column: string; direction: "ASC" | "DESC" } | null;

type Props = {
  host: DatabaseAdminHost;
  /** Opens schema.sql in the editor. Absent where the app's files are not in the editor. */
  onEditSchema?: () => void;
  onExportDatabase: () => void;
  target?: "local" | "deployed";
  deployedLabel?: string;
  onTargetChange?: (target: "local" | "deployed") => void;
};

export function DatabaseStudio({ host, onEditSchema, onExportDatabase, target = "local", deployedLabel, onTargetChange }: Props) {
  const [tab, setTab] = useState<StudioTab>("data");
  const [tables, setTables] = useState<DatabaseTable[]>([]);
  const [tablesReady, setTablesReady] = useState(false);
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
    } finally {
      setTablesReady(true);
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
        {/* Which rows this is comes first: an empty table of test rows must never read as a live app with none. */}
        {onTargetChange && (
          <Segmented
            label="Which rows"
            value={target}
            onChange={onTargetChange}
            options={[{ value: "deployed", label: "Live app" }, { value: "local", label: "Test rows in this browser" }]}
          />
        )}
        <TabBar
          label="Database views"
          className="data-views"
          value={tab}
          onChange={setTab}
          items={[
            { id: "data", label: "Rows", icon: <Table2 size={13} aria-hidden="true" /> },
            { id: "sql", label: "SQL", icon: <Code2 size={13} aria-hidden="true" /> },
            { id: "schema", label: "Schema", icon: <FileCode2 size={13} aria-hidden="true" /> },
            { id: "backups", label: "Backups", icon: <History size={13} aria-hidden="true" /> },
          ]}
        />
      </header>
      {target === "deployed" ? (
        <div className="live-database-banner">
          <Badge tone="warning">Live</Badge>
          <span>These are the rows of {deployedLabel ? <strong>{deployedLabel}</strong> : "the published app"}. A change here reaches the people using it right away.</span>
        </div>
      ) : (
        <p className="test-rows-note">{onTargetChange ? "Test rows the preview uses, kept in this browser. The live app has its own." : "Test rows the preview uses, kept in this browser. The app gets rows of its own when you deploy it."}</p>
      )}

      {tab === "data" && (
        <DataBrowser
          host={host}
          tables={tables}
          tablesReady={tablesReady}
          selectedTable={selectedTable}
          onSelectTable={setSelectedTable}
          refreshVersion={refreshVersion}
          onMutated={mutated}
          onExport={onExportDatabase}
        />
      )}
      {tab === "sql" && <SqlConsole host={host} tables={tables} onMutated={mutated} />}
      {tab === "schema" && <SchemaView tables={tables} onEdit={onEditSchema} />}
      {tab === "backups" && <BackupsView host={host} tables={tables} deployed={target === "deployed"} onExport={onExportDatabase} onRestored={mutated} />}
    </section>
  );
}

function DataBrowser({ host, tables, tablesReady, selectedTable, onSelectTable, refreshVersion, onMutated, onExport }: {
  host: DatabaseAdminHost;
  tables: DatabaseTable[];
  tablesReady: boolean;
  selectedTable: string;
  onSelectTable: (table: string) => void;
  refreshVersion: number;
  onMutated: () => void;
  onExport: () => void;
}) {
  return (
    <div className="data-browser">
      <aside className="data-table-sidebar">
        <div className="data-sidebar-label"><span>TABLES</span><span>{tablesReady ? tables.length : "…"}</span></div>
        <div className="data-sidebar-list">
          {tables.map((table) => (
            <button key={table.name} className={selectedTable === table.name ? "active" : ""} onClick={() => onSelectTable(table.name)}>
              <Table2 size={12} /><span><strong>{table.name}</strong><small>{table.columns.length} columns</small></span><em>{table.totalRows}</em>
            </button>
          ))}
        </div>
        <div className="data-sidebar-note"><Database size={13} /><p>This app's rows stay in their own database.</p></div>
      </aside>
      <div className="data-grid-region">
        {selectedTable ? (
          <EditableTable
            key={selectedTable}
            host={host}
            tableName={selectedTable}
            refreshVersion={refreshVersion}
            onMutated={onMutated}
            onExport={onExport}
          />
        ) : !tablesReady ? (
          <StudioLoading label="Reading tables" />
        ) : (
          <StudioEmpty title={tables.length === 0 ? "No tables yet" : "No rows yet."} detail={tables.length === 0 ? "Open Schema and add the first table. The file updates for people who edit code." : "Pick a table to see its rows."} />
        )}
      </div>
    </div>
  );
}

/**
 * The grid keeps its own sort and page, so a mutation refreshes it through a
 * dependency rather than through the key: remounting on every edited cell threw
 * away the order and the page the owner had chosen, with no sign that it had.
 */
function EditableTable({ host, tableName, refreshVersion, onMutated, onExport }: {
  host: DatabaseAdminHost;
  tableName: string;
  /** Bumped by any write, here or in the SQL console: the page is re-read, not rebuilt. */
  refreshVersion: number;
  onMutated: () => void;
  onExport: () => void;
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
        <div><p>TABLE</p><h3>{tableName === "tasks" ? "Tasks" : tableName}</h3><span>Stops at 100 rows.</span></div>
        <div className="table-toolbar-actions">
          <button onClick={() => void load()}><RefreshCw size={12} /> Refresh</button>
          <button className="insert-row-button" onClick={openInsert}><Plus size={12} /> Insert row</button>
          <button onClick={onExport}><Download size={12} /> Download rows</button>
        </div>
      </div>

      {error && <div className="table-error" role="alert"><span>{error}</span><button aria-label="Dismiss error" onClick={() => setError("")}><X size={12} /></button></div>}

      {insertOpen && data && (
        <div className="insert-row-form">
          <div className="insert-title"><div><p>NEW ROW</p><strong>Insert into {tableName}</strong></div><button aria-label="Close new row form" onClick={() => setInsertOpen(false)}><X size={14} /></button></div>
          <div className="insert-fields">
            {data.columns.filter((column) => !column.primaryKey).map((column) => (
              <label key={column.name}><span>{columnLabel(column.name)}<em>{column.type}</em></span><input value={insertValues[column.name] ?? ""} placeholder={column.defaultValue !== null ? `default: ${String(column.defaultValue)}` : column.notNull ? "required" : "null"} onChange={(event) => setInsertValues((current) => ({ ...current, [column.name]: event.target.value }))} /></label>
            ))}
          </div>
          <div className="insert-actions"><button onClick={() => setInsertOpen(false)}>Cancel</button><button className="primary" onClick={() => void commitInsert()}><Plus size={12} /> Insert row</button></div>
        </div>
      )}

      <div className="editable-grid-wrap">
        {loading ? <StudioLoading label="Reading the table" /> : !data ? <EmptyColumns names={tableName === "tasks" ? ["title", "done", "created_at"] : []} /> : data.rows.length === 0 ? <EmptyColumns names={data.columns.map((column) => column.name).filter((name) => name !== "id")} /> : (
          <table className="editable-grid">
            <thead><tr><th className="grid-row-number">#</th>{data.columns.filter((column) => column.name !== "id").map((column) => (
              <th key={column.name}><button onClick={() => toggleSort(column.name)}><span>{columnLabel(column.name)}</span>{sort?.column === column.name ? sort.direction === "ASC" ? <ChevronUp size={12} /> : <ChevronDown size={12} /> : null}</button></th>
            ))}<th className="grid-actions-heading" /></tr></thead>
            <tbody>{data.rows.map((row, rowIndex) => (
              <tr key={String(row.rowId)}>
                <td className="grid-row-number">{page * 50 + rowIndex + 1}</td>
                {data.columns.filter((column) => column.name !== "id").map((column) => {
                  const value = row.values[column.name];
                  const isEditing = editing?.rowId === row.rowId && editing.column === column.name;
                  return <td key={column.name} className={isEditing ? "editing" : ""}>{isEditing ? (
                    <div className="cell-editor"><input autoFocus value={editing.value} onChange={(event) => setEditing({ ...editing, value: event.target.value })} onKeyDown={(event) => { if (event.key === "Enter") void commitEdit(); if (event.key === "Escape") setEditing(null); }} /><button aria-label="Save cell" onClick={() => void commitEdit()}><Check size={11} /></button></div>
                  ) : (
                    <button className={`cell-value ${value === null ? "is-null" : ""}`} onClick={() => setEditing({ rowId: row.rowId, column: column.name, value: value === null ? "" : String(value) })}>{value === null ? "null" : formatCellText(value)}<Pencil size={9} /></button>
                  )}</td>;
                })}
                <td className="grid-row-actions">{deleteRowId === row.rowId ? <div><button className="confirm-delete" onClick={() => void commitDelete()}>Delete</button><button aria-label="Keep this row" onClick={() => setDeleteRowId(null)}><X size={11} /></button></div> : <button aria-label={`Delete row ${rowIndex + 1}`} onClick={() => setDeleteRowId(row.rowId)}><Trash2 size={12} /></button>}</td>
              </tr>
            ))}</tbody>
          </table>
        )}
      </div>

      {pages > 1 && <footer className="table-pagination">
        <span>Page {page + 1} of {pages}</span>
        <div><button disabled={page === 0} onClick={() => setPage((current) => current - 1)}><ChevronLeft size={13} /> Prev</button><button disabled={page >= pages - 1} onClick={() => setPage((current) => current + 1)}>Next <ChevronRight size={13} /></button></div>
      </footer>}
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
  const [lastRead, setLastRead] = useState(true);
  const [confirming, setConfirming] = useState<string | null>(null);
  const editorSchema = useMemo(() => Object.fromEntries(tables.map((table) => [table.name, table.columns])), [tables]);

  const run = async (confirmed = false) => {
    if (!query.trim() || running) return;
    const kind = classifySql(query);
    // Destructive statements ask first, because only a backup can undo them.
    if (kind.danger && !confirmed) {
      setConfirming(kind.danger);
      return;
    }
    setConfirming(null);
    setRunning(true);
    setError("");
    const started = performance.now();
    try {
      const next = await host.runSql(query);
      setResult(next);
      setLastRead(kind.read);
      setShowHistory(false);
      setHistory((current) => [{ sql: query, elapsedMs: next.elapsedMs, ok: true }, ...current].slice(0, 30));
      // DDL reports no changed rows, yet it changes the table list the most.
      if (!kind.read || next.changes > 0) onMutated();
    } catch (cause) {
      setResult(null);
      setError(cause instanceof Error ? cause.message : String(cause));
      setHistory((current) => [{ sql: query, elapsedMs: performance.now() - started, ok: false }, ...current].slice(0, 30));
      // A script can write in its first statements and fail in a later one.
      if (!kind.read) onMutated();
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="standalone-sql">
      <div className="sql-page-heading"><div><h3>Stops at 100 rows</h3><span>A list that reads rows needs a limit. This box already stops there.</span></div></div>
      <div className="sql-surface">
        <div className="sql-query-title"><span><i /> Query</span><button onClick={() => void run()} aria-busy={running || undefined}><Play size={12} fill="currentColor" /> {running ? "Running…" : "Run query"}</button></div>
        <div className="standalone-editor"><SqlEditor value={query} schema={editorSchema} onChange={setQuery} onExecute={() => void run()} /></div>
        <div className="sql-result-tabs" role="tablist" aria-label="Query output">
          <button type="button" role="tab" aria-selected={!showHistory} className={!showHistory ? "active" : ""} onClick={() => setShowHistory(false)}>Results</button>
          <button type="button" role="tab" aria-selected={showHistory} className={showHistory ? "active" : ""} onClick={() => setShowHistory(true)}>History <span>{history.length}</span></button>
          {result && !showHistory && <small>{lastRead || result.columns.length > 0 ? `${result.rows.length} row${result.rows.length === 1 ? "" : "s"}` : `${result.changes} row${result.changes === 1 ? "" : "s"} affected`} · {result.elapsedMs.toFixed(1)} ms</small>}
        </div>
        <div className="standalone-results">{showHistory ? <QueryHistory history={history} onSelect={(sql) => { setQuery(sql); setShowHistory(false); }} /> : <QueryResult result={result} error={error} read={lastRead} />}</div>
      </div>
      <Dialog
        open={confirming !== null}
        title="Run this statement?"
        description={confirming ?? ""}
        onClose={() => setConfirming(null)}
        footer={<>
          <Button onClick={() => setConfirming(null)}>Cancel</Button>
          <Button variant="danger" onClick={() => void run(true)}>Run statement</Button>
        </>}
      >
        <pre className="sql-confirm-preview"><code>{query.trim()}</code></pre>
      </Dialog>
    </div>
  );
}

function QueryResult({ result, error, read }: { result: DatabaseRunResult | null; error: string; read: boolean }) {
  if (error) return <div className="sql-error" role="alert"><strong>SQLite error</strong><code>{error}</code></div>;
  if (!result) return <StudioEmpty title="Nothing yet" detail="Run the query above. The rows show up here." />;
  // An empty read has no result set at all in SQLite, so it has no column names either.
  if (result.columns.length === 0 && read) return <table className="data-table"><thead><tr><th className="row-index">#</th><th>Result</th></tr></thead><tbody><tr><td className="row-index" /><td className="data-table-empty">0 rows</td></tr></tbody></table>;
  if (result.columns.length === 0) return <div className="sql-success"><strong>Query executed</strong><span>{result.changes} row{result.changes === 1 ? "" : "s"} affected</span></div>;
  return <table className="data-table"><thead><tr><th className="row-index">#</th>{result.columns.map((column) => <th key={column}>{column}</th>)}</tr></thead><tbody>{result.rows.map((row, index) => <tr key={index}><td className="row-index">{index + 1}</td>{result.columns.map((column) => <td key={column}>{formatCellText(row[column])}</td>)}</tr>)}</tbody></table>;
}

function QueryHistory({ history, onSelect }: { history: Array<{ sql: string; elapsedMs: number; ok: boolean }>; onSelect: (sql: string) => void }) {
  if (history.length === 0) return <StudioEmpty title="No query history" detail="Queries executed in this session will appear here." />;
  return <div className="query-history">{history.map((item, index) => <button key={`${item.sql}-${index}`} onClick={() => onSelect(item.sql)}><i className={item.ok ? "ok" : "error"} /><code>{item.sql}</code><span>{item.elapsedMs.toFixed(1)} ms</span></button>)}</div>;
}

/**
 * The tables the database holds now. schema.sql is the model: a publish runs
 * it, so a new table or index arrives, and a changed column needs the owner's
 * ALTER TABLE in the SQL view.
 */
function SchemaView({ tables, onEdit }: { tables: DatabaseTable[]; onEdit?: () => void }) {
  return (
    <div className="studio-document-view schema-model-view">
      <header>
        <div><p>schema.sql</p><h3>{tables.length === 0 ? "No tables yet." : `${tables.length} table${tables.length === 1 ? "" : "s"} in this database.`}</h3></div>
      </header>
      <ul className="schema-columns">
        {tables.map((table) => (
          <li key={table.name}><strong>{table.name}</strong><span>{table.columns.join(", ")}</span></li>
        ))}
      </ul>
      {onEdit
        ? <div className="schema-actions"><button onClick={onEdit}><Pencil size={12} /> Edit schema.sql</button></div>
        : <p className="schema-rule">This app's schema.sql is in the folder it was published from. Edit it there and publish.</p>}
      <p className="schema-rule">A publish runs schema.sql, so a new table or index arrives with it. A changed or removed column does not arrive, so change it with ALTER TABLE in SQL, here and on the published app.</p>
    </div>
  );
}

function BackupsView({ host, tables, deployed, onExport, onRestored }: {
  host: DatabaseAdminHost;
  tables: DatabaseTable[];
  deployed: boolean;
  onExport: () => void;
  onRestored: () => void;
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
      await host.createDatabaseBackup("Manual backup");
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
      await host.restoreDatabaseBackup(restoreId);
      setRestoreId("");
      // A restore writes a backup of what it replaced, so the list grows.
      await loadBackups();
      onRestored();
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
        <div><h3>{deployed ? "Rows of the published app" : "Test rows in this browser"}</h3><span>Export, under More, takes the accounts, the rows and the rules.</span></div>
        {supportsRestore
          ? <button onClick={() => void createBackup()} disabled={busy}><History size={12} /> Create backup</button>
          : <button onClick={onExport}><Download size={12} /> Download rows</button>}
      </header>
      <div className="backup-card">
        <span className="backup-icon"><Clock3 size={24} /></span>
        <div><small>{tables.length} table{tables.length === 1 ? "" : "s"} · {rowCount} row{rowCount === 1 ? "" : "s"}</small></div>
        {rowCount > 0 && <button onClick={onExport}><Download size={13} /> Download rows</button>}
      </div>
      {message && <div className="backup-message" role="status">{message}</div>}
      {error && <div className="backup-error" role="alert">{error}</div>}
      {supportsRestore ? (
        backups.length > 0 ? (
          <ul className="backup-list" aria-label="Database backups">
            {backups.map((backup) => (
              <li key={backup.id}>
                <div><strong>{backup.label}</strong><small>{new Date(backup.createdAt).toLocaleString()} · {formatBytes(backup.sizeBytes)}{backup.holds?.length ? ` · ${backupHolds(backup.holds)}` : ""}</small></div>
                {restoreId === backup.id ? (
                  <div className="backup-confirm"><span>{backup.holds?.length ? `Replace the app's ${backupHolds(backup.holds)}?` : "Replace the current database?"}</span><button onClick={() => void restoreBackup()} disabled={busy}>Confirm restore</button><button onClick={() => setRestoreId("")} disabled={busy}>Cancel</button></div>
                ) : <button onClick={() => setRestoreId(backup.id)} disabled={busy}>Restore</button>}
              </li>
            ))}
          </ul>
        ) : <div className="future-backups"><History size={20} /><div><strong>No restore points yet</strong><p>{deployed ? "Create a backup before changing the tables. The published app keeps each one in its folder on the machine that serves it." : "Create a backup before changing the tables. This preview keeps five. A published copy keeps its backups on the machine that hosts it."}</p></div></div>
      ) : <div className="future-backups"><History size={20} /><div><strong>Managed restore is not available for this target yet</strong><p>Download a consistent SQLite snapshot before changing the deployed database.</p></div></div>}
    </div>
  );
}

/** What a backup taken before an import brings back, in words. */
function backupHolds(holds: string[]) {
  const data = holds.includes("data"), sources = holds.includes("sources");
  return data && sources ? "data and saved sources" : data ? "data, accounts and files" : "saved sources";
}

function columnLabel(name: string): string {
  if (name === "id") return "Id";
  if (name === "title") return "Title";
  if (name === "done") return "Done";
  if (name === "created_at") return "Created";
  const words = name.replaceAll("_", " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

function EmptyColumns({ names }: { names: string[] }) {
  const columns = names.length > 0 ? names : ["title", "done", "created_at"];
  return (
    <div className="empty-rows-surface">
      <table className="editable-grid empty-columns">
        <thead><tr><th className="grid-row-number">#</th>{columns.map((name) => <th key={name}>{columnLabel(name)}</th>)}</tr></thead>
      </table>
      <div className="empty-rows-invite">
        <strong>No rows yet.</strong>
        <p>Insert a row to start.</p>
      </div>
    </div>
  );
}

function StudioLoading({ label }: { label: string }) {
  return <div className="studio-loading"><LoadingBlock label={label} rows={5} /></div>;
}

function StudioEmpty({ title, detail, loading = false }: { title: string; detail: string; loading?: boolean }) {
  return <EmptyState className="studio-empty" icon={loading ? <RefreshCw className="spin" size={22} /> : <Table2 size={22} />} title={title} description={detail} />;
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
