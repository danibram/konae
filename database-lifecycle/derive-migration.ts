/**
 * Turns two Database Models into the SQL statements that move one to the other.
 *
 * This file is the only place a Capsule's `schema.ts` is executed, and it is meant
 * to be run as a separate process (see `deriveStatements` in server.ts) — never
 * imported into the Kernel. ADR-0007: the Kernel process never evaluates a
 * Capsule's source, because it is the process that holds every Deployment's
 * database and the identity store.
 *
 * Diffing runs here too, for a second reason: Drizzle's differ asks a human when a
 * change is ambiguous, and it decides whether to ask by looking at the terminal.
 * Here there is provably no terminal — stdin is a pipe the parent has already
 * closed — so the question can never be posed to a process that cannot answer it.
 *
 * Protocol: a JSON request on stdin, a framed JSON reply on stdout.
 */
import { randomUUID } from "node:crypto";
import { rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { text } from "node:stream/consumers";
import { build } from "esbuild";
import { generateSQLiteDrizzleJson, generateSQLiteMigration } from "drizzle-kit/api";
import { assertConstrainedModel, repoRoot } from "./model-guard";
import { assertPlannableChange } from "./plannable-change";

/** Framing token: the model may print anything, so the reply is what follows the last one. */
export const replyMarker = "<<littlebox-schema-reply>>";

export type DeriveRequest = { acceptedSource: string; candidateSource: string };
export type DeriveReply = { ok: true; statements: string[] } | { ok: false; error: string };

export async function statementsFor(acceptedSource: string, candidateSource: string): Promise<string[]> {
  const [previous, next] = await Promise.all([
    snapshotFromSource(acceptedSource, "accepted schema.ts"),
    snapshotFromSource(candidateSource, "candidate schema.ts"),
  ]);
  // Refuse an ambiguous change with a message the owner can act on, rather than
  // letting Drizzle discover it has nobody to ask.
  assertPlannableChange(previous, next);
  return await generateSQLiteMigration(previous as never, next as never);
}

export async function snapshotFromSource(source: string, label: string): Promise<unknown> {
  // The syntax allowlist catches honest mistakes early. On a reachable Kernel
  // the process that imports the model has already dropped to a uid that
  // cannot read /data. A loopback Kernel may run this as itself.
  assertConstrainedModel(source, label);

  const result = await build({
    stdin: { contents: source, loader: "ts", resolveDir: repoRoot(), sourcefile: `${label}.ts` },
    bundle: true,
    platform: "node",
    format: "esm",
    target: "node20",
    write: false,
    logLevel: "silent",
  });
  const code = result.outputFiles[0]?.text;
  if (!code) throw new Error(`Could not compile ${label}.`);

  // A data: URL would be tidier, but Bun rejects one this large as a path name.
  // The file lives in this process's own temp directory and is removed after the
  // import, whatever the model does.
  const file = path.join(os.tmpdir(), `littlebox-model-${randomUUID()}.mjs`);
  await writeFile(file, code, "utf8");
  try {
    const model = await import(`file://${file}`) as Record<string, unknown>;
    return await generateSQLiteDrizzleJson(model);
  } finally {
    await rm(file, { force: true });
  }
}

async function main(): Promise<void> {
  const reply = await (async (): Promise<DeriveReply> => {
    try {
      const request = JSON.parse(await text(process.stdin)) as DeriveRequest;
      return { ok: true, statements: await statementsFor(request.acceptedSource, request.candidateSource) };
    } catch (cause) {
      return { ok: false, error: cause instanceof Error ? cause.message : String(cause) };
    }
  })();
  // Anything the evaluated model wrote to stdout would corrupt the reply, so the
  // reply is framed and the parent reads only what follows the last marker.
  process.stdout.write(`\n${replyMarker}${JSON.stringify(reply)}\n`);
}

// Runs only when this file is the process entry; importing it must evaluate nothing.
const entry = process.argv[1] ? path.resolve(process.argv[1]) : "";
if (entry && import.meta.url === `file://${entry}`) await main();
