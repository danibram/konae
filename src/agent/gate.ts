import type { Database, SqlJsStatic } from "sql.js";
import { parseServerActionSource, resolveServerAction, type ServerActionDefinition, type ServerActionProgram } from "../kernel/actions";
import { firstSqlKeyword } from "../runtime/security";
import type {
  CapsuleFiles,
  CapsuleValidator,
  LiveDatabase,
  ValidationError,
  ValidationResult,
  ValidationStage,
} from "./contracts";

export type SqlJsLoader = () => Promise<SqlJsStatic>;

let browserSql: Promise<SqlJsStatic> | undefined;

/**
 * The browser loader: the runtime host's own memoised sql.js, so one tab holds
 * one wasm heap instead of two. The import is dynamic so the gate stays usable
 * in plain runtimes (tests, a future Kernel-side gate) with an injected loader.
 * A failed load is never memoised: a transient wasm fetch failure must not
 * disable the Validation Gate for the rest of the session.
 */
export const loadBrowserSqlJs: SqlJsLoader = () => {
  browserSql ??= import("../runtime/host")
    .then((module) => module.loadSqlJs())
    .catch((error) => {
      browserSql = undefined;
      throw error;
    });
  return browserSql;
};

export function createCapsuleValidator(loadSql: SqlJsLoader = loadBrowserSqlJs): CapsuleValidator {
  return new SqlJsCapsuleValidator(loadSql);
}

/**
 * The Validation Gate. Three stages, all in-process on sql.js:
 *
 * 1. `schema`   — schema.sql must execute on a fresh database, twice: the
 *                 runtime re-runs it on every database open, so a
 *                 non-idempotent statement kills the app on the next load. It
 *                 must also only build, never destroy: it is the one file that
 *                 runs against the owner's live data.
 * 2. `actions`  — actions.json must parse, every binding must resolve, and every
 *                 statement must prepare against the schema it declares.
 * 3. `migration`— only when the live database could be read: schema.sql must
 *                 also run on a clone of it, the clone must end up carrying every
 *                 table and column the fresh schema declares, and every action
 *                 must prepare against that clone. Authoring a real migration
 *                 script is a later slice; v1 relies on the idempotent
 *                 `CREATE TABLE IF NOT EXISTS` schema and reports divergence.
 *
 * The result names the stages that actually ran, because a stage that was
 * skipped must never be reported as one that held.
 */
export class SqlJsCapsuleValidator implements CapsuleValidator {
  constructor(private readonly loadSql: SqlJsLoader = loadBrowserSqlJs) {}

  async validate(files: CapsuleFiles, live: LiveDatabase = { state: "absent" }): Promise<ValidationResult> {
    const SQL = await this.loadSql();
    const errors: ValidationError[] = [];
    const checked: ValidationStage[] = [];
    let fresh: Database | undefined;
    let clone: Database | undefined;

    try {
      fresh = new SQL.Database();
      try {
        fresh.run(files["schema.sql"]);
      } catch (error) {
        return failure({
          stage: "schema",
          message: "schema.sql does not run on an empty database.",
          detail: describe(error),
        });
      }

      // The runtime re-runs schema.sql on every database open, so running once proves nothing.
      try {
        fresh.run(files["schema.sql"]);
      } catch (error) {
        return failure({
          stage: "schema",
          message: "schema.sql is not idempotent: it fails when it is run a second time.",
          detail: `${describe(error)} — schema.sql runs again on every database open, so use CREATE TABLE IF NOT EXISTS / CREATE INDEX IF NOT EXISTS and never ALTER an existing table here.`,
        });
      }

      const destructive = destructiveStatements(fresh, files["schema.sql"]);
      if (destructive.length > 0) {
        return failure({
          stage: "schema",
          message: `schema.sql may only create structure and seed rows, but it starts a statement with ${destructive.join(", ")}.`,
          detail:
            "schema.sql is re-run against the owner's live database on every open, so a statement that removes or rewrites what is " +
            "already there deletes real data. The migration stage cannot catch it either: that runs on a copy. Express the change as " +
            "CREATE TABLE IF NOT EXISTS / CREATE INDEX IF NOT EXISTS, and leave data changes to server actions.",
        });
      }
      checked.push("schema");

      let program: ServerActionProgram | undefined;
      try {
        program = parseServerActionSource(files["actions.json"]);
      } catch (error) {
        errors.push({ stage: "actions", message: "actions.json is not a valid server action program.", detail: describe(error) });
      }

      if (program) errors.push(...checkActions(fresh, program, "actions"));
      checked.push("actions");

      if (live.state === "unreadable") {
        // Not the same as having no database: there is one, and the gate cannot
        // see it. Passing here would promise a convergence nobody checked.
        errors.push({
          stage: "migration",
          message: "The current database could not be read, so the migration stage could not run.",
          detail: live.reason,
        });
      }

      if (live.state === "loaded") {
        try {
          clone = new SQL.Database(live.bytes);
        } catch (error) {
          errors.push({ stage: "migration", message: "The current database export could not be opened.", detail: describe(error) });
        }
        if (clone) {
          let migrated = true;
          try {
            clone.run(files["schema.sql"]);
          } catch (error) {
            migrated = false;
            errors.push({
              stage: "migration",
              message: "schema.sql does not run on the current database.",
              detail: describe(error),
            });
          }
          if (migrated) {
            errors.push(...divergence(fresh, clone));
            if (program) errors.push(...checkActions(clone, program, "migration"));
          }
          checked.push("migration");
        }
      }
    } finally {
      fresh?.close();
      clone?.close();
    }

    return { ok: errors.length === 0, errors, checked };
  }
}

/**
 * Statements schema.sql is not allowed to contain, by their leading keyword.
 * An allowlist rather than a denylist: this file runs against real data, so a
 * keyword nobody thought about must be refused, not waved through.
 */
const schemaKeywords = new Set(["CREATE", "INSERT", "PRAGMA", "BEGIN", "COMMIT", "END", "SAVEPOINT", "RELEASE"]);

/**
 * The leading keywords of any statement schema.sql may not run. SQLite itself
 * splits the script — a naive split on ";" would cut a trigger body in half and
 * read its UPDATE as a top-level statement.
 */
function destructiveStatements(fresh: Database, schema: string): string[] {
  const found = new Set<string>();
  try {
    for (const statement of fresh.iterateStatements(schema)) {
      const keyword = firstSqlKeyword(statement.getSQL());
      statement.free();
      if (keyword && !schemaKeywords.has(keyword)) found.add(keyword);
    }
  } catch {
    // The script already ran twice, so it parses; if this walk cannot repeat that
    // it says nothing about the statements, and inventing an error would be worse.
    return [];
  }
  return [...found];
}

function checkActions(db: Database, program: ServerActionProgram, stage: "actions" | "migration"): ValidationError[] {
  const target = stage === "actions" ? "the schema in schema.sql" : "the current database";
  const errors: ValidationError[] = [];

  for (const action of Object.values(program.actions)) {
    try {
      const statement = db.prepare(action.statement);
      statement.free();
    } catch (error) {
      errors.push({
        stage,
        message: `Server action ${action.name} does not prepare against ${target}.`,
        detail: describe(error),
      });
      continue;
    }

    if (stage !== "actions") continue;

    try {
      resolveServerAction(program, action.name, syntheticInput(action), { actorId: "validation_actor", now: 0 });
    } catch (error) {
      errors.push({
        stage,
        message: `The bindings of server action ${action.name} cannot be resolved.`,
        detail: describe(error),
      });
      continue;
    }

    const mismatch = placeholderMismatch(db, action);
    if (mismatch) errors.push({ stage, message: `Server action ${action.name} ${mismatch}`, detail: action.statement });
  }

  return errors;
}

/** Tables and columns the fresh schema declares but the migrated clone does not carry. */
function divergence(fresh: Database, clone: Database): ValidationError[] {
  const errors: ValidationError[] = [];
  const cloneTables = new Set(userTables(clone));

  for (const table of userTables(fresh)) {
    if (!cloneTables.has(table)) {
      errors.push({
        stage: "migration",
        message: `Table ${table} is declared in schema.sql but missing from the current database.`,
      });
      continue;
    }
    const existing = new Set(tableColumns(clone, table));
    const missing = tableColumns(fresh, table).filter((column) => !existing.has(column));
    if (missing.length > 0) {
      errors.push({
        stage: "migration",
        message: `Table ${table} in the current database is missing: ${missing.join(", ")}.`,
        detail: "CREATE TABLE IF NOT EXISTS cannot add columns to an existing table, so this change needs a migration before it can be applied.",
      });
    }
  }

  return errors;
}

/**
 * sql.js reports "column index out of range" when more values are bound than the
 * statement has placeholders, which turns binding into an exact parameter count
 * probe. Null is not usable as a probe value; 1 is.
 */
function placeholderMismatch(db: Database, action: ServerActionDefinition): string | null {
  const declared = action.bindings.length;
  if (!accepts(db, action.statement, declared)) {
    return `declares ${declared} binding${declared === 1 ? "" : "s"} but its statement takes fewer parameters.`;
  }
  if (accepts(db, action.statement, declared + 1)) {
    return `declares ${declared} binding${declared === 1 ? "" : "s"} but its statement takes more parameters.`;
  }
  return null;
}

function accepts(db: Database, sql: string, count: number): boolean {
  const statement = db.prepare(sql);
  try {
    statement.bind(Array.from({ length: count }, () => 1));
    return true;
  } catch {
    return false;
  } finally {
    statement.free();
  }
}

function syntheticInput(action: ServerActionDefinition): Record<string, unknown> {
  const input: Record<string, unknown> = {};
  for (const binding of action.bindings) {
    if (binding.source !== "input") continue;
    input[binding.key] = binding.type === "string" ? "x" : binding.type === "number" ? 1 : true;
  }
  return input;
}

function userTables(db: Database): string[] {
  return db
    .exec("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")[0]
    ?.values.flat().map(String) ?? [];
}

function tableColumns(db: Database, table: string): string[] {
  const escaped = table.replaceAll('"', '""');
  return db.exec(`PRAGMA table_info("${escaped}")`)[0]?.values.map((row) => String(row[1])) ?? [];
}

/** A schema that does not hold stops the run: nothing after it was checked. */
function failure(error: ValidationError): ValidationResult {
  return { ok: false, errors: [error], checked: [] };
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
