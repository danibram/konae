import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import path from "node:path";
import { accessSync, constants } from "node:fs";
import { text } from "node:stream/consumers";
import type { DatabaseMigrationPlan, PlanDatabaseChangeInput } from "../src/database-lifecycle/contracts";
import { replyMarker } from "./derive-migration";
import { repoRoot } from "./model-guard";

/**
 * Planning a migration is two jobs with two risk profiles (ADR-0007): deriving
 * the statements runs the owner's `schema.ts` and Drizzle's differ, so it is
 * exiled to a separate process; deciding what those statements mean for a
 * Deployment is Konae's judgement over plain data, so it stays here.
 */
export async function planDrizzleDatabaseChange(input: PlanDatabaseChangeInput): Promise<DatabaseMigrationPlan> {
  const statements = await deriveStatements(input);
  if (statements.length === 0) throw new Error("The Drizzle model has no schema changes to migrate.");

  const fromHash = hash(input.acceptedSource);
  const toHash = hash(input.candidateSource);
  const sql = statements.join("\n--> statement-breakpoint\n");
  return {
    id: hash(`${fromHash}:${toHash}:${sql}`),
    fromHash,
    toHash,
    sql,
    statements,
    destructive: statements.some((statement) => /\b(DROP\s+TABLE|DROP\s+COLUMN|DELETE\s+FROM)\b/i.test(statement)),
  };
}

const derivationTimeoutMs = 15_000;
const maximumReplyBytes = 4_000_000;

/**
 * Runs the derivation as its own process. The child inherits **no environment**,
 * so a model that defeats the static guard still finds no admin key, no data
 * directory and no deployment hostname to read; it is killed on a timeout, and
 * its reply is size-capped.
 */
async function deriveStatements(input: PlanDatabaseChangeInput): Promise<string[]> {
  const child = spawn(
    bunExecutable(),
    ["run", path.join(repoRoot(), "database-lifecycle", "derive-migration.ts")],
    {
      // The child resolves its own imports from the repository root, so it does
      // not need — and must not get — the Kernel's working directory.
      cwd: repoRoot(),
      env: {},
      stdio: ["pipe", "pipe", "ignore"],
    },
  );

  await new Promise<void>((resolve, reject) => {
    child.once("spawn", resolve);
    child.once("error", reject);
  });
  child.stdin.on("error", () => {});
  child.stdin.end(JSON.stringify(input));
  const timeout = setTimeout(() => child.kill("SIGKILL"), derivationTimeoutMs);

  let raw: string;
  try {
    raw = await text(child.stdout);
  } finally {
    clearTimeout(timeout);
  }

  if (raw.length > maximumReplyBytes) throw new Error("The schema change produced too large a result.");
  const marker = raw.lastIndexOf(replyMarker);
  // No framed reply means the process died — a timeout, a crash, or a model that
  // tried to leave. None of those is a change Konae can migrate to.
  if (marker < 0) throw new Error("Could not derive a migration from these schema.ts files.");

  // The reply crosses a process boundary, so it is read as data to be checked,
  // not as a type to be trusted.
  const reply = JSON.parse(raw.slice(marker + replyMarker.length)) as { ok?: unknown; statements?: unknown; error?: unknown };
  if (reply.ok !== true) {
    throw new Error(typeof reply.error === "string" ? reply.error : "Could not derive a migration from these schema.ts files.");
  }
  if (!Array.isArray(reply.statements) || reply.statements.some((statement) => typeof statement !== "string")) {
    throw new Error("Could not derive a migration from these schema.ts files.");
  }
  return reply.statements as string[];
}

function hash(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

function bunExecutable(): string {
  if (process.versions.bun) return process.execPath;
  if (process.env.KONAE_BUN_PATH) return process.env.KONAE_BUN_PATH;
  for (const directory of (process.env.PATH ?? "").split(path.delimiter)) {
    const candidate = path.join(directory, "bun");
    try {
      accessSync(candidate, constants.X_OK);
      return candidate;
    } catch { /* Continue through the parent's executable search path. */ }
  }
  throw new Error("Bun is required to generate migrations. Start the editor with bun run dev:local.");
}
