import type { Database, SqlJsStatic } from "sql.js";
import { notifyColumnRefusal, notifyReturning, notifyReturningStatement, parseServerActionSource, resolveServerAction, type ServerActionDefinition, type ServerActionProgram } from "../kernel/actions";
import { sqlStatementTokens, sqlWordsOf, type SqlToken } from "../lib/sql-tokens";
import { contractFromProgram } from "../lib/action-contract";
import { entryNeverRunsMessage, rootRelativeWarnings } from "../lib/static-artifact.js";
import { releaseRefusal, type ReleaseFacts } from "./release-check";
import { hintOf, ownerFixes, refusalOf } from "../kernel/refusal";
import { checkModuleChange } from "../lib/module-state";
import { sdkRequired, sourcesUseActions, sourcesUseAuth, sourcesUseFiles } from "../lib/module-use";
import { checkSourceFiles } from "../lib/source-rules";
import { SCHEMA_TRANSFORMS_FILE, schemaTransformsRefusal } from "./schema-transforms";
import { isEncoded } from "../lib/source-files";
import type {
  CapsuleFiles,
  CapsuleValidator,
  LiveDatabase,
  ValidationError,
  ValidationResult,
  ValidationStage,
  ValidationWarning,
  ValidationFacts,
} from "./contracts";

export { ownerFixes, releaseRefusal, type ReleaseFacts };

export type SqlJsLoader = () => Promise<SqlJsStatic>;

let browserSql: Promise<SqlJsStatic> | undefined;

export const loadBrowserSqlJs: SqlJsLoader = () => {
  browserSql ??= import("../runtime/host")
    .then((module) => module.loadSqlJs())
    .catch((error) => {
      browserSql = undefined;
      throw error;
    });
  return browserSql;
};

const RELEASE_OWNER = "Deploy refuses this until the owner sets the secret in Access > Integrations. No edit to the files clears it.";
const PREVIEW_BEHIND = "The preview's test database is behind schema.sql, so the preview cannot run the actions that need the change. Publishing applies it to the published database, so try the change on konae dev or a published Deployment.";
const RELEASE_AUTHOR = "The Kernel refuses this at publish. Change actions.json.";

export function createCapsuleValidator(loadSql: SqlJsLoader = loadBrowserSqlJs): CapsuleValidator {
  return new SqlJsCapsuleValidator(loadSql);
}

export class SqlJsCapsuleValidator implements CapsuleValidator {
  constructor(private readonly loadSql: SqlJsLoader = loadBrowserSqlJs) {}

  async validate(files: CapsuleFiles, live: LiveDatabase = { state: "absent" }, facts: ValidationFacts = {}): Promise<ValidationResult> {
    try {
      checkProposalSources(files, facts.sourceFiles);
    } catch (error) {
      const refused = refusalOf(error);
      return failure({
        stage: "sources",
        message: describe(error),
        ...(refused ? { code: refused.code, hint: hintOf(refused.code) } : {}),
      });
    }
    const SQL = await this.loadSql();
    const schemaSource = files["schema.sql"] ?? "";
    const actionsSource = files["actions.json"] ?? `{"version":1,"actions":{}}`;
    const filesCall = sourcesUseFiles(Object.values(files));
    const actionsState = facts.modules?.actions;
    const actionsOff = actionsState ? checkModuleChange({ rule: "on", module: "actions", state: actionsState }) : null;
    if (sourcesUseActions(Object.values(files)) && actionsOff) {
      return failure({ stage: "actions", message: actionsOff.message, code: "module.off", hint: hintOf("module.off") }, ["sources"]);
    }
    const filesState = facts.modules?.files;
    const filesOff = filesState ? checkModuleChange({ rule: "on", module: "files", state: filesState }) : null;
    if (filesCall && filesOff) {
      return failure({ stage: "actions", message: filesOff.message, code: filesOff.code as "module.off", hint: hintOf(filesOff.code as "module.off") }, ["sources"]);
    }
    const authState = facts.modules?.auth;
    if (facts.visibility === "private" && authState && authState.on !== true) {
      const required = checkModuleChange({ rule: "private-auth", requiredState: authState });
      if (required) return failure({ stage: "actions", message: required.message, code: required.code as "module.requires", hint: hintOf(required.code as "module.requires") }, ["sources"]);
    }
    if (filesState?.on === true && authState && authState.on !== true) {
      const required = checkModuleChange({ rule: "requires", module: "files", required: "auth", requiredState: authState });
      if (required) return failure({ stage: "actions", message: required.message, code: "module.requires", hint: hintOf("module.requires") }, ["sources"]);
    }
    const authOff = authState ? checkModuleChange({ rule: "on", module: "auth", state: authState }) : null;
    if (sourcesUseAuth(Object.values(files)) && authOff) {
      return failure({ stage: "actions", message: authOff.message, code: authOff.code as "module.off", hint: hintOf(authOff.code as "module.off") }, ["sources"]);
    }
    const errors: ValidationError[] = [];
    const checked: ValidationStage[] = ["sources"];
    const warnings: ValidationWarning[] = [];
    let fresh: Database | undefined;
    let clone: Database | undefined;

    try {
      const transforms = files[SCHEMA_TRANSFORMS_FILE];
      if (typeof transforms === "string") {
        const refused = schemaTransformsRefusal(transforms);
        if (refused) return failure({ stage: "schema", message: refused.message, code: refused.code, hint: refused.hint }, checked);
        const definition = JSON.parse(transforms) as { transitions: unknown[] };
        if (definition.transitions.length > 0) {
          warnings.push({
            stage: "schema",
            message: "The Validation Gate does not inspect applied history, verify the database schema fingerprint, measure affected row counts, or enforce runtime constraints. Run konae rehearse or konae dev, or test a Deployment.",
          });
        }
      }
      const refused = refusedStatements(schemaSource);
      if (refused.length > 0) {
        return failure({
          stage: "schema",
          code: "schema.statement-refused",
          hint: hintOf("schema.statement-refused"),
          message: `schema.sql may only create tables and indexes, but it contains ${refused.join(", ")}.`,
          detail:
            "The Kernel applies schema.sql with an authorizer that allows CREATE TABLE and CREATE [UNIQUE] INDEX and nothing else, " +
            "and refuses the functions that load extensions or read and write host files, so any of these fails at deploy. " +
            "schema.sql states what the database should be, and the Kernel derives the change to the published database from it. Rows change through server actions.",
        }, checked);
      }

      fresh = new SQL.Database();
      try {
        fresh.run(schemaSource);
      } catch (error) {
        return failure({
          stage: "schema",
          message: "schema.sql does not run on an empty database.",
          detail: describe(error),
        }, checked);
      }

      try {
        fresh.run(schemaSource);
      } catch (error) {
        return failure({
          stage: "schema",
          message: "schema.sql is not idempotent: it fails when it is run a second time.",
          detail: `${describe(error)}. The preview runs schema.sql again on every open, so use CREATE TABLE IF NOT EXISTS / CREATE INDEX IF NOT EXISTS.`,
        }, checked);
      }
      const reserved = fresh.exec("SELECT name FROM sqlite_master WHERE lower(name) IN ('__konae_schema_state','__konae_schema_history') ORDER BY lower(name), name LIMIT 1")[0]?.values[0]?.[0];
      if (typeof reserved === "string") return failure({
        stage: "schema", code: "schema.statement-refused", hint: hintOf("schema.statement-refused"),
        message: `schema.sql cannot create the reserved name ${reserved}.`,
      }, checked);
      checked.push("schema");

      let program: ServerActionProgram | undefined;
      try {
        program = parseServerActionSource(actionsSource);
      } catch (error) {
        const refused = refusalOf(error);
        errors.push({ stage: "actions", message: "actions.json is not a valid server action program.", detail: describe(error), ...(refused ? { code: refused.code, hint: refused.hint } : {}) });
      }

      if (program) errors.push(...checkActions(fresh, program, "actions"));
      const release = program && errors.length === 0 ? releaseRefusal(program, facts) : null;
      if (release && ownerFixes(release.code)) warnings.push({ stage: "actions", message: release.message, detail: RELEASE_OWNER, owner: true, code: release.code, hint: release.hint });
      else if (release) errors.push({ stage: "actions", message: release.message, detail: RELEASE_AUTHOR, code: release.code, hint: release.hint });
      if (program) warnings.push(...contractFromProgram(program, schemaSource).warnings.map(({ message }) => ({ stage: "actions" as const, message })));
      checked.push("actions");

      if (live.state === "unreadable") {
        errors.push({
          stage: "preview-rows",
          message: "The current database could not be read, so the preview-rows stage could not run.",
          detail: live.reason,
        });
      }

      if (live.state === "loaded") {
        try {
          clone = new SQL.Database(live.bytes);
        } catch (error) {
          errors.push({ stage: "preview-rows", message: "The current database export could not be opened.", detail: describe(error) });
        }
        if (clone) {
          let behind: string[];
          try {
            clone.run(schemaSource);
            behind = divergence(fresh, clone);
          } catch (error) {
            behind = [`schema.sql does not run on it: ${describe(error)}.`];
          }
          if (behind.length > 0) warnings.push({ stage: "preview-rows", message: PREVIEW_BEHIND, detail: behind.join(" ") });
          else {
            if (program) errors.push(...checkActions(clone, program, "preview-rows"));
            checked.push("preview-rows");
          }
        }
      }
    } finally {
      fresh?.close();
      clone?.close();
    }

    const sdk = sdkRequired(facts.modules, Object.values(files), facts.visibility);
    const neverRuns = entryNeverRunsMessage(Object.keys(files), files, sdk);
    if (neverRuns) errors.push({ stage: "actions", message: neverRuns });
    warnings.push(...rootRelativeWarnings(files).map(({ message }) => ({ stage: "actions" as const, message })));
    return { ok: errors.length === 0, errors, checked, ...(warnings.length > 0 ? { warnings } : {}) };
  }
}

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

const refusedFunctionNames = new Set(["load_extension", "readfile", "writefile", "edit", "fts3_tokenizer"]);

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

function refusedCreate(tokens: SqlToken[], words: string[]): string | null {
  const object = words[1] === "UNIQUE" ? words[2] : words[1];
  if (object !== "INDEX" && object !== "TABLE") return `CREATE ${words[1] ?? ""}`.trim();

  const start = tokens.findIndex((token) => token.kind === "word" && token.text.toUpperCase() === object);
  let index = start + 1;
  if (isWord(tokens[index], "IF")) index += 3;
  const schema = tokens[index + 1]?.kind === "symbol" && (tokens[index + 1] as { text: string }).text === "." ? nameOf(tokens[index]) : null;
  if (schema && ["temp", "temporary"].includes(schema.toLowerCase())) return `CREATE ${object} ${schema}.*`;
  if (object === "INDEX") return null;

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

function checkActions(db: Database, program: ServerActionProgram, stage: "actions" | "preview-rows"): ValidationError[] {
  const target = stage === "actions" ? "the schema in schema.sql" : "the current database";
  const errors: ValidationError[] = [];

  for (const action of Object.values(program.actions)) {
    if (action.kind === "signal" || action.kind === "call") {
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

function divergence(fresh: Database, clone: Database): string[] {
  const found: string[] = [];
  const cloneTables = new Set(userTables(clone));
  for (const table of userTables(fresh)) {
    if (!cloneTables.has(table)) {
      found.push(`It has no table ${table}.`);
      continue;
    }
    const existing = new Set(tableColumns(clone, table));
    const missing = tableColumns(fresh, table).filter((column) => !existing.has(column));
    if (missing.length > 0) found.push(`Its table ${table} has no ${missing.join(", ")}.`);
  }
  return found;
}

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

function failure(error: ValidationError, checked: ValidationStage[] = []): ValidationResult {
  return { ok: false, errors: [error], checked };
}

function checkProposalSources(files: CapsuleFiles, sourceFiles?: ValidationFacts["sourceFiles"]) {
  if (!sourceFiles?.encodings) {
    checkSourceFiles({ files });
    return;
  }
  const retained = Object.keys(sourceFiles.encodings).filter((path) => !(path in files) && isEncoded(sourceFiles, path));
  const binaryFiles = Object.fromEntries(retained.map((path) => [path, sourceFiles.files[path]]));
  const encodings = Object.fromEntries(retained.map((path) => [path, "base64"]));
  checkSourceFiles({ files: { ...binaryFiles, ...files }, encodings });
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
