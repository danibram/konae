import type { Database, SqlJsStatic } from "sql.js";
import { notifyColumnRefusal, notifyReturning, notifyReturningStatement, parseServerActionSource, resolveServerAction, type ServerActionDefinition, type ServerActionProgram } from "../kernel/actions";
import { sqlStatementTokens, sqlWordsOf, type SqlToken } from "../lib/sql-tokens";
import { contractFromProgram } from "../lib/action-contract";
import { rootRelativeWarnings } from "../lib/static-artifact.js";
import { releaseRefusal, type ReleaseFacts } from "./release-check";
import { hintOf, ownerFixes, refusalOf } from "../kernel/refusal";
import type {
  CapsuleFiles,
  CapsuleValidator,
  LiveDatabase,
  ValidationError,
  ValidationResult,
  ValidationStage,
  ValidationWarning,
} from "./contracts";

export { ownerFixes, releaseRefusal, type ReleaseFacts };

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

/** Who clears a release refusal, beside the Kernel's sentence. */
const RELEASE_OWNER = "Deploy refuses this until the owner sets the secret in Auth > Integrations. No edit to the files clears it.";
const RELEASE_AUTHOR = "The Kernel refuses this at publish. Change actions.json.";

export function createCapsuleValidator(loadSql: SqlJsLoader = loadBrowserSqlJs): CapsuleValidator {
  return new SqlJsCapsuleValidator(loadSql);
}

/**
 * The Validation Gate. Three stages, all in-process on sql.js:
 *
 * 1. `schema`   — schema.sql must execute on a fresh database, twice: the
 *                 runtime re-runs it on every database open, so a
 *                 non-idempotent statement kills the app on the next load. It
 *                 may only hold CREATE TABLE and CREATE [UNIQUE] INDEX, the
 *                 same allowlist the Kernel's schema authorizer enforces at
 *                 deploy: it is the one file that runs against the owner's
 *                 live data.
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

  async validate(files: CapsuleFiles, live: LiveDatabase = { state: "absent" }, facts: ReleaseFacts = {}): Promise<ValidationResult> {
    const SQL = await this.loadSql();
    const errors: ValidationError[] = [];
    const checked: ValidationStage[] = [];
    const warnings: ValidationWarning[] = [];
    let fresh: Database | undefined;
    let clone: Database | undefined;

    try {
      // Read before running: sql.js lacks some refused functions, and "no such
      // function" would hide the reason the Kernel gives.
      const refused = refusedStatements(files["schema.sql"]);
      if (refused.length > 0) {
        return failure({
          stage: "schema",
          code: "schema.statement-refused",
          hint: hintOf("schema.statement-refused"),
          message: `schema.sql may only create tables and indexes, but it contains ${refused.join(", ")}.`,
          detail:
            "The Kernel applies schema.sql with an authorizer that allows CREATE TABLE and CREATE [UNIQUE] INDEX and nothing else, " +
            "and refuses the functions that load extensions or read and write host files, so any of these fails at deploy. schema.sql is also re-run against the owner's live database on every open, so a " +
            "statement that writes rows would rewrite real data each time. Use CREATE TABLE IF NOT EXISTS / CREATE INDEX IF NOT EXISTS, " +
            "and leave data changes to server actions.",
        });
      }

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
          detail: `${describe(error)}. schema.sql runs again on every database open, so use CREATE TABLE IF NOT EXISTS / CREATE INDEX IF NOT EXISTS and never ALTER an existing table here.`,
        });
      }
      checked.push("schema");

      let program: ServerActionProgram | undefined;
      try {
        program = parseServerActionSource(files["actions.json"]);
      } catch (error) {
        // A refusal of the Capsule keeps the Kernel's code beside its sentence (ADR-0020).
        const refused = refusalOf(error);
        errors.push({ stage: "actions", message: "actions.json is not a valid server action program.", detail: describe(error), ...(refused ? { code: refused.code, hint: refused.hint } : {}) });
      }

      if (program) errors.push(...checkActions(fresh, program, "actions"));
      // The Kernel parses before it plans a release, so its release sentence only follows a program that parsed.
      const release = program && errors.length === 0 ? releaseRefusal(program, facts) : null;
      // A secret only the owner can set does not block the change: the files are
      // right, and Deploy still refuses until the owner sets it.
      if (release && ownerFixes(release.code)) warnings.push({ stage: "actions", message: release.message, detail: RELEASE_OWNER, owner: true, code: release.code, hint: release.hint });
      else if (release) errors.push({ stage: "actions", message: release.message, detail: RELEASE_AUTHOR, code: release.code, hint: release.hint });
      // A result column without a name a second backend could match passes, with a warning.
      if (program) warnings.push(...contractFromProgram(program, files["schema.sql"]).warnings.map(({ message }) => ({ stage: "actions" as const, message })));
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

    // A root-relative URL leaves the app's path address. The build warns with the same words, and the Kernel has no release warning to match.
    warnings.push(...rootRelativeWarnings(files).map(({ message }) => ({ stage: "actions" as const, message })));
    return { ok: errors.length === 0, errors, checked, ...(warnings.length > 0 ? { warnings } : {}) };
  }
}

/**
 * What schema.sql may not run, named for the error. The allowlist matches the
 * Kernel's schema authorizer (kernel-rs/src/main.rs, `apply_additive_schema`):
 * CREATE TABLE and CREATE [UNIQUE] INDEX, nothing else. INSERT, PRAGMA and
 * transaction statements run fine in sql.js but are refused at deploy. SQL
 * functions are allowed (a CHECK, an index expression, a partial index, a
 * generated column), except the ones that reach outside the database. The
 * script is split by the tokenizer rather than by preparing each statement:
 * preparing a second CREATE TABLE fails once the table exists, and a naive
 * split on ";" would cut a trigger body in half.
 */
function refusedStatements(schema: string): string[] {
  const found = new Set<string>();
  for (const tokens of sqlStatementTokens(schema)) {
    const words = sqlWordsOf(tokens);
    if (words.length === 0) {
      found.add("a statement that is not CREATE TABLE or CREATE INDEX");
      continue;
    }
    const refused = words[0] === "CREATE" ? refusedCreate(tokens, words) : words[0];
    if (refused) found.add(refused);
    for (const name of refusedFunctions(tokens)) found.add(`${name}()`);
  }
  return [...found];
}

/**
 * Functions the Kernel's authorizer refuses by name: they load native code or
 * read and write files on the host. sql.js does not ship most of them, so the
 * gate has to name them rather than wait for the statement to fail.
 */
const refusedFunctionNames = new Set(["load_extension", "readfile", "writefile", "fts3_tokenizer"]);

/** The refused functions `tokens` call. A quoted name still calls the function. */
function refusedFunctions(tokens: SqlToken[]): string[] {
  const found: string[] = [];
  tokens.forEach((token, index) => {
    if (token.kind !== "word" && token.kind !== "identifier") return;
    const next = tokens[index + 1];
    const name = token.text.toLowerCase();
    if (next?.kind === "symbol" && next.text === "(" && refusedFunctionNames.has(name)) found.push(name);
  });
  return found;
}

/**
 * The CREATE forms the Kernel will not deploy. A trigger or a view passes
 * sql.js but fails the authorizer, and a trigger could delete rows whenever
 * something writes to its table. CREATE TABLE ... AS (a SELECT, a VALUES list
 * or a WITH query) copies rows into the new table, which the authorizer
 * refuses as an INSERT. TEMP objects are a different authorizer action and are
 * refused too, whether the statement says TEMP or names the temp schema.
 */
function refusedCreate(tokens: SqlToken[], words: string[]): string | null {
  const object = words[1] === "UNIQUE" ? words[2] : words[1];
  if (object !== "INDEX" && object !== "TABLE") return `CREATE ${words[1] ?? ""}`.trim();

  const start = tokens.findIndex((token) => token.kind === "word" && token.text.toUpperCase() === object);
  let index = start + 1;
  // IF NOT EXISTS
  if (isWord(tokens[index], "IF")) index += 3;
  const schema = tokens[index + 1]?.kind === "symbol" && (tokens[index + 1] as { text: string }).text === "." ? nameOf(tokens[index]) : null;
  if (schema && ["temp", "temporary"].includes(schema.toLowerCase())) return `CREATE ${object} ${schema}.*`;
  if (object === "INDEX") return null;

  // The column list opens with the first top-level "(". An AS before it is a
  // CREATE TABLE ... AS query; GENERATED ALWAYS AS sits inside the list.
  const outer = tokens[start].depth;
  for (let at = index; at < tokens.length; at += 1) {
    const token = tokens[at];
    if (token.depth !== outer) continue;
    if (token.kind === "symbol" && token.text === "(") return null;
    if (isWord(token, "AS")) return "CREATE TABLE ... AS";
  }
  return null;
}

function isWord(token: SqlToken | undefined, word: string): boolean {
  return token?.kind === "word" && token.text.toUpperCase() === word;
}

function nameOf(token: SqlToken | undefined): string | null {
  return token && (token.kind === "word" || token.kind === "identifier") ? token.text : null;
}

function checkActions(db: Database, program: ServerActionProgram, stage: "actions" | "migration"): ValidationError[] {
  const target = stage === "actions" ? "the schema in schema.sql" : "the current database";
  const errors: ValidationError[] = [];

  for (const action of Object.values(program.actions)) {
    if (action.kind === "signal" || action.kind === "call") {
      // A signal action and a call run no SQL; their bindings still have to resolve.
      if (stage !== "actions") continue;
      try {
        resolveServerAction(program, action.name, syntheticInput(action), { actorId: "validation_actor", now: 0 });
      } catch (error) {
        errors.push({ stage, message: `The bindings of server action ${action.name} cannot be resolved.`, detail: describe(error) });
      }
      continue;
    }
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

    const refused = notifyColumnsRefusal(db, action);
    if (refused) errors.push({ stage, message: refused.message, detail: refused.message, code: refused.code, hint: hintOf(refused.code) });

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

/**
 * A notify column the statement cannot return, checked by preparing the
 * statement with the Kernel's own RETURNING clause, as kernel-rs/src/notify.rs
 * `check_columns` does at publish (ADR-0021).
 */
export function notifyColumnsRefusal(db: Database, action: ServerActionDefinition) {
  const columns = action.kind === "mutation" ? notifyReturning(action.notify) : [];
  if (!columns.length) return undefined;
  try {
    const statement = db.prepare(notifyReturningStatement(action.statement, columns));
    const returned = statement.getColumnNames().length;
    statement.free();
    if (returned === columns.length) return undefined;
  } catch {
    // Falls through to the refusal.
  }
  return notifyColumnRefusal(action.name);
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
